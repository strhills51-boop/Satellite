//! Content hashes of files, with a cache that skips files unchanged since the last time. @sat:scan.content-hash
//!
//! Owns: the hash of a file's bytes (xxh3, 64 bits), and the hash cache — a text file of `hash size mtime path`
//! lines; a line counts while the file keeps its size and modification time.
//! Not here: what is compared (`@sat:identical`), folder signatures for the scan cache (`@sat:scan.signature`).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;

/// The hash of a file's bytes.
pub fn hash_file(path: &Path) -> std::io::Result<u64> {
    let mut f = std::fs::File::open(path)?;
    let mut h = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.digest())
}

fn stamp(path: &Path) -> Option<(u64, u128)> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((md.len(), t))
}

// @sat:scan.content-hash.cache
/// Hashes kept between runs.
#[derive(Debug, Default)]
pub struct HashCache {
    entries: HashMap<PathBuf, (u64, u64, u128)>,
    dirty: bool,
}

impl HashCache {
    pub fn load(file: &Path) -> Self {
        let mut entries = HashMap::new();
        if let Ok(text) = std::fs::read_to_string(file) {
            for line in text.lines() {
                let mut it = line.splitn(4, ' ');
                let (Some(h), Some(s), Some(t), Some(p)) = (it.next(), it.next(), it.next(), it.next())
                else {
                    continue;
                };
                if let (Ok(h), Ok(s), Ok(t)) = (u64::from_str_radix(h, 16), s.parse(), t.parse()) {
                    entries.insert(PathBuf::from(p), (h, s, t));
                }
            }
        }
        Self {
            entries,
            dirty: false,
        }
    }

    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        let mut lines: Vec<String> = self
            .entries
            .iter()
            .map(|(p, (h, s, t))| format!("{h:016x} {s} {t} {}", p.display()))
            .collect();
        lines.sort_unstable();
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = file.with_extension("tmp");
        std::fs::write(&tmp, lines.join("\n"))?;
        std::fs::rename(tmp, file)
    }

    /// Hashes of `paths` (in their order; `None` — not readable), from the cache where the file is unchanged, else
    /// read in parallel. Returns how many were read.
    pub fn hash_all(&mut self, paths: &[PathBuf]) -> (Vec<Option<u64>>, usize) {
        let stamps: Vec<Option<(u64, u128)>> = paths.par_iter().map(|p| stamp(p)).collect();
        let fresh: Vec<Option<u64>> = paths
            .iter()
            .zip(&stamps)
            .map(|(p, st)| {
                let (s, t) = (*st)?;
                self.entries.get(p).filter(|e| e.1 == s && e.2 == t).map(|e| e.0)
            })
            .collect();
        let todo: Vec<usize> = (0..paths.len())
            .filter(|&i| fresh[i].is_none() && stamps[i].is_some())
            .collect();
        let read: Vec<(usize, Option<u64>)> =
            todo.par_iter().map(|&i| (i, hash_file(&paths[i]).ok())).collect();
        let mut out = fresh;
        for (i, h) in &read {
            out[*i] = *h;
            if let (Some(h), Some((s, t))) = (h, stamps[*i]) {
                self.entries.insert(paths[*i].clone(), (*h, s, t));
                self.dirty = true;
            }
        }
        (out, read.len())
    }
}

// @sat:scan.content-hash.table
/// Content hashes for a provider table (`@sat:identical.needs` decides which loose copies are read; the rest stay
/// unknown): `folder(mod name)` — where the mod's files are. Hashes are taken from and kept in `cache_file`.
/// Returns the hashes and how many files were read now.
pub fn hashes_for(
    table: &sat_core::table::ProviderTable,
    folder: impl Fn(&str) -> PathBuf,
    cache_file: &Path,
    min_size: u64,
) -> (sat_core::identical::ContentHashes, usize) {
    let need = sat_core::identical::needs_hash(table, min_size);
    let names = table.mod_names();
    let paths: Vec<PathBuf> = need
        .iter()
        .map(|&i| folder(&names[table.owner(i).idx()]).join(table.path_string(table.path_of_provider(i))))
        .collect();
    let mut cache = HashCache::load(cache_file);
    let (hashes, read) = cache.hash_all(&paths);
    let _ = cache.save(cache_file);
    let mut out = vec![None; table.provider_count()];
    for (i, h) in need.into_iter().zip(hashes) {
        out[i] = h;
    }
    (sat_core::identical::ContentHashes(out), read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_cached_while_files_stay_the_same() {
        let d = std::env::temp_dir().join(format!("sat-content-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (a, b, c) = (d.join("a bin"), d.join("b"), d.join("c"));
        std::fs::write(&a, b"same").unwrap();
        std::fs::write(&b, b"same").unwrap();
        std::fs::write(&c, b"diff").unwrap();
        let paths = vec![a.clone(), b.clone(), c.clone(), d.join("missing")];
        let mut cache = HashCache::default();
        let (h, read) = cache.hash_all(&paths);
        assert_eq!(read, 3);
        assert!(h[0] == h[1] && h[0] != h[2] && h[3].is_none());
        let file = d.join("cache").join("hashes.txt");
        cache.save(&file).unwrap();
        let mut again = HashCache::load(&file);
        let (h2, read) = again.hash_all(&paths);
        assert_eq!(
            (h2, read),
            (h, 0),
            "nothing read again: the paths with spaces survive the file too"
        );
        std::fs::write(&c, b"changed!").unwrap();
        assert_eq!(again.hash_all(&paths).1, 1, "a changed file is read again");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
