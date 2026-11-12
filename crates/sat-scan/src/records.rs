//! A plugin's records read from disk, cached while the plugin is unchanged. @sat:scan.records
//!
//! Owns: reading a plugin into record facts (`@sat:records`: FormID, signature, hash of the unpacked data) and the
//! record cache — one binary file per plugin in a cache folder, valid while the plugin keeps its size and time.
//! Not here: the record index and what it shows (`@sat:records.index`), parsing (`@sat:formats.plugin.records`).

use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use sat_core::records::{PluginRecords, RecordFact};

const MAGIC: &[u8; 4] = b"SRC1";

fn stamp(path: &Path) -> Option<(u64, u128)> {
    let md = std::fs::metadata(path).ok()?;
    Some((
        md.len(),
        md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos(),
    ))
}

/// Reads a plugin's records.
pub fn read(path: &Path) -> Result<PluginRecords, String> {
    let open = || std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()));
    let header = sat_formats::read_plugin_header(BufReader::new(open()?))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut records = Vec::with_capacity(header.num_records as usize);
    sat_formats::read_records(BufReader::with_capacity(1 << 20, open()?), |r| {
        records.push(RecordFact {
            form_id: r.form_id,
            signature: r.signature,
            hash: r.data.map(xxhash_rust::xxh3::xxh3_64),
        });
    })
    .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(PluginRecords {
        masters: header.masters,
        records,
    })
}

fn cache_file(dir: &Path, plugin: &Path) -> PathBuf {
    let key = xxhash_rust::xxh3::xxh3_64(plugin.to_string_lossy().to_lowercase().as_bytes());
    dir.join(format!("{key:016x}.rec"))
}

fn encode(st: (u64, u128), r: &PluginRecords) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&st.0.to_le_bytes());
    out.extend_from_slice(&st.1.to_le_bytes());
    out.extend_from_slice(&(r.masters.len() as u32).to_le_bytes());
    for m in &r.masters {
        out.extend_from_slice(&(m.len() as u16).to_le_bytes());
        out.extend_from_slice(m.as_bytes());
    }
    out.extend_from_slice(&(r.records.len() as u32).to_le_bytes());
    for f in &r.records {
        out.extend_from_slice(&f.form_id.to_le_bytes());
        out.extend_from_slice(&f.signature);
        out.push(u8::from(f.hash.is_some()));
        out.extend_from_slice(&f.hash.unwrap_or(0).to_le_bytes());
    }
    out
}

fn decode(b: &[u8], st: (u64, u128)) -> Option<PluginRecords> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = b.get(at..at + n)?;
        at += n;
        Some(s)
    };
    if take(4)? != MAGIC {
        return None;
    }
    let size = u64::from_le_bytes(take(8)?.try_into().ok()?);
    let time = u128::from_le_bytes(take(16)?.try_into().ok()?);
    if (size, time) != st {
        return None;
    }
    let n = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
    let mut masters = Vec::with_capacity(n);
    for _ in 0..n {
        let len = u16::from_le_bytes(take(2)?.try_into().ok()?) as usize;
        masters.push(String::from_utf8(take(len)?.to_vec()).ok()?);
    }
    let n = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
    let mut records = Vec::with_capacity(n);
    for _ in 0..n {
        let r = take(17)?;
        records.push(RecordFact {
            form_id: u32::from_le_bytes(r[0..4].try_into().ok()?),
            signature: r[4..8].try_into().ok()?,
            hash: (r[8] == 1).then(|| u64::from_le_bytes(r[9..17].try_into().expect("8 bytes"))),
        });
    }
    Some(PluginRecords { masters, records })
}

// @sat:scan.records.load
/// Records of `plugins` (in their order), from `cache_dir` where a plugin is unchanged, else read in parallel and
/// cached. Returns the records (an unreadable plugin — its error) and how many plugins were read now.
pub fn load_all(plugins: &[PathBuf], cache_dir: &Path) -> (Vec<Result<PluginRecords, String>>, usize) {
    let _ = std::fs::create_dir_all(cache_dir);
    let out: Vec<(Result<PluginRecords, String>, bool)> = plugins
        .par_iter()
        .map(|p| {
            let Some(st) = stamp(p) else {
                return (Err(format!("{}: not found", p.display())), false);
            };
            let cf = cache_file(cache_dir, p);
            let mut buf = Vec::new();
            if std::fs::File::open(&cf)
                .and_then(|mut f| f.read_to_end(&mut buf))
                .is_ok()
                && let Some(r) = decode(&buf, st)
            {
                return (Ok(r), false);
            }
            let r = read(p);
            if let Ok(rec) = &r
                && let Ok(mut f) = std::fs::File::create(&cf)
            {
                let _ = f.write_all(&encode(st, rec));
            }
            (r, true)
        })
        .collect();
    let read = out.iter().filter(|x| x.1).count();
    (out.into_iter().map(|x| x.0).collect(), read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_are_read_once_and_cached() {
        let d = std::env::temp_dir().join(format!("sat-records-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("A.esp");
        std::fs::write(
            &p,
            sat_formats::plugin::fixtures::plugin(
                false,
                &["Skyrim.esm"],
                &[
                    (b"NPC_", 0x0001_2E49, b"npc", false),
                    (b"LVLI", 0x0100_0801, b"list", true),
                ],
            ),
        )
        .unwrap();
        let cache = d.join("cache");
        let (r, read) = load_all(std::slice::from_ref(&p), &cache);
        let r = r[0].as_ref().unwrap().clone();
        assert_eq!(
            (read, r.masters.clone(), r.records.len()),
            (1, vec!["Skyrim.esm".to_string()], 2)
        );
        assert!(r.records.iter().all(|f| f.hash.is_some()));
        let (again, read) = load_all(std::slice::from_ref(&p), &cache);
        assert_eq!((read, again[0].as_ref().unwrap()), (0, &r), "from the cache");
        let (missing, _) = load_all(&[d.join("nope.esp")], &cache);
        assert!(missing[0].is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
