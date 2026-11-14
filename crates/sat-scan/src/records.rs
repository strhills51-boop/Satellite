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

/// v2: the hash covers the record flags too (a flag change, e.g. persistent or disabled, is a change); v3: the flags
/// themselves are kept (deleted references, `@sat:conflict.kinds`).
const MAGIC: &[u8; 4] = b"SRC3";

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
            flags: r.flags,
            // the data and the record flags (not "compressed": the same record packed or not is the same)
            hash: r.data.map(|d| {
                let mut h = xxhash_rust::xxh3::Xxh3::new();
                h.update(&(r.flags & !0x0004_0000).to_le_bytes());
                h.update(d);
                h.digest()
            }),
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
        out.extend_from_slice(&f.flags.to_le_bytes());
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
        let r = take(21)?;
        records.push(RecordFact {
            form_id: u32::from_le_bytes(r[0..4].try_into().ok()?),
            signature: r[4..8].try_into().ok()?,
            flags: u32::from_le_bytes(r[8..12].try_into().ok()?),
            hash: (r[12] == 1).then(|| u64::from_le_bytes(r[13..21].try_into().expect("8 bytes"))),
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

// @sat:scan.records.loaded
/// The files of the loaded plugins, in load order: base masters from the game's `data` folder, the rest from the
/// folder of the mod they load from (`folder(mod name)`).
pub fn loaded_plugins(
    list: &sat_core::plugins::PluginList,
    table: &sat_core::table::ProviderTable,
    folder: impl Fn(&str) -> PathBuf,
    data: &Path,
) -> Vec<(String, PathBuf)> {
    let names = table.mod_names();
    list.plugins
        .iter()
        .filter(|p| p.active)
        .filter_map(|p| {
            let path = match (p.origin, p.winner) {
                (sat_core::plugins::Origin::Base, _) => data.join(&p.name),
                (_, Some(w)) => folder(&names[p.versions[w].owner.idx()]).join(&p.name),
                _ => return None,
            };
            Some((p.name.clone(), path))
        })
        .collect()
}

/// Reads (or takes from the cache) the loaded plugins and builds the record index; plugins not read come back with
/// why. Returns the index, those plugins, and how many plugins were read now.
pub fn index(
    loaded: &[(String, PathBuf)],
    cache_dir: &Path,
) -> (sat_core::records::RecordIndex, Vec<String>, usize) {
    let paths: Vec<PathBuf> = loaded.iter().map(|x| x.1.clone()).collect();
    let (recs, read) = load_all(&paths, cache_dir);
    let mut ok = Vec::new();
    let mut unread = Vec::new();
    for ((name, _), r) in loaded.iter().zip(&recs) {
        match r {
            Ok(r) => ok.push((name.clone(), r)),
            Err(e) => unread.push(format!("{name}: {e}")),
        }
    }
    (sat_core::records::RecordIndex::build(&ok), unread, read)
}

/// A field's hash with references mapped to their plugins: every aligned 4-byte word whose top byte names one of
/// the plugin's masters (or itself) is hashed as (that plugin, object id), so the same reference through different
/// master lists hashes the same.
fn field_hash(bytes: &[u8], map: &[u32]) -> u64 {
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    h.update(&(bytes.len() as u32).to_le_bytes());
    if bytes.len().is_multiple_of(4) {
        for w in bytes.as_chunks::<4>().0 {
            let v = u32::from_le_bytes(*w);
            match map.get((v >> 24) as usize) {
                // object ids below 0x800 are the engine's own (and most small numbers): they stay raw
                Some(&o) if v & 0x00FF_FFFF >= 0x800 => {
                    h.update(&[1]);
                    h.update(&o.to_le_bytes());
                    h.update(&(v & 0x00FF_FFFF).to_le_bytes());
                }
                _ => {
                    h.update(&[0]);
                    h.update(w);
                }
            }
        }
    } else {
        h.update(bytes);
    }
    h.digest()
}

// @sat:scan.records.fields
/// Fields of the conflict records (`@sat:conflict.fields`): every plugin of every record two overriders share is
/// read again, only those records kept.
pub fn fields(
    loaded: &[(String, PathBuf)],
    ix: &sat_core::records::RecordIndex,
) -> sat_core::conflict_fields::FieldFacts {
    use std::collections::HashMap;
    let mut wanted: Vec<HashMap<u32, u64>> = vec![HashMap::new(); ix.plugins.len()];
    for (key, plugins) in ix.field_targets() {
        for p in plugins {
            if let Some(raw) = ix.raw_form_id(p, key) {
                wanted[p as usize].insert(raw, key);
            }
        }
    }
    // the index holds only the plugins that were read: find each one's file by name
    let path_of: HashMap<String, &PathBuf> = loaded.iter().map(|(n, p)| (n.to_lowercase(), p)).collect();
    let parts: Vec<Vec<((u32, u64), sat_core::conflict_fields::Fields)>> = ix
        .plugins
        .par_iter()
        .enumerate()
        .filter(|(i, _)| !wanted[*i].is_empty())
        .filter_map(|(i, n)| path_of.get(&n.to_lowercase()).map(|p| (i, *p)))
        .map(|(i, path)| {
            let want = &wanted[i];
            let map = ix.origin_map(i as u32);
            let mut out = Vec::new();
            let Ok(f) = std::fs::File::open(path) else {
                return out;
            };
            let _ = sat_formats::read_records(BufReader::with_capacity(1 << 20, f), |r| {
                let Some(&key) = want.get(&r.form_id) else { return };
                let Some(d) = r.data else { return };
                let fields = sat_formats::subrecords(d)
                    .into_iter()
                    .map(|(sig, b)| (sig, field_hash(b, &map)))
                    .collect();
                out.push(((i as u32, key), fields));
            });
            out
        })
        .collect();
    sat_core::conflict_fields::FieldFacts(parts.into_iter().flatten().collect())
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

#[cfg(test)]
mod field_tests {
    use super::*;

    #[test]
    fn a_reference_through_different_master_lists_hashes_the_same() {
        // plugin 1 has [A, B] as masters, plugin 2 has [B]: B's object 0x1234 is 01001234 in one, 00001234 in the other
        let one = field_hash(&0x0100_1234u32.to_le_bytes(), &[10, 11, 12]);
        let two = field_hash(&0x0000_1234u32.to_le_bytes(), &[11, 13]);
        assert_eq!(one, two);
        assert_ne!(one, field_hash(&0x0000_1234u32.to_le_bytes(), &[10, 13]));
        // small numbers stay numbers
        assert_eq!(
            field_hash(&5u32.to_le_bytes(), &[10]),
            field_hash(&5u32.to_le_bytes(), &[11])
        );
    }
}
