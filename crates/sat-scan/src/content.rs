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
/// Hashes kept between runs: files by path, archive entries as `archive::entry` with the archive's size and time;
/// `None` — an entry that cannot be compared (kept too, so the archive is not read again for it).
#[derive(Debug, Default)]
pub struct HashCache {
    entries: HashMap<PathBuf, (Option<u64>, u64, u128)>,
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
                let h = if h == "-" {
                    Ok(None)
                } else {
                    u64::from_str_radix(h, 16).map(Some)
                };
                if let (Ok(h), Ok(s), Ok(t)) = (h, s.parse(), t.parse()) {
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
            .map(|(p, (h, s, t))| match h {
                Some(h) => format!("{h:016x} {s} {t} {}", p.display()),
                None => format!("- {s} {t} {}", p.display()),
            })
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
                self.entries
                    .get(p)
                    .filter(|e| e.1 == s && e.2 == t)
                    .and_then(|e| e.0)
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
                self.entries.insert(paths[*i].clone(), (Some(*h), s, t));
                self.dirty = true;
            }
        }
        (out, read.len())
    }
}

fn entry_key(archive: &Path, entry: &str) -> PathBuf {
    PathBuf::from(format!("{}::{entry}", archive.display()))
}

impl HashCache {
    // @sat:scan.content-hash.archive
    /// Hashes of `entries` of one archive (unpacked bytes, `@sat:formats.archive-data`), from the cache while the
    /// archive is unchanged. `None` — not comparable or not readable. Returns whether the archive was read.
    pub fn hash_archive(&mut self, archive: &Path, entries: &[String]) -> (Vec<Option<u64>>, bool) {
        let Some((s, t)) = stamp(archive) else {
            return (vec![None; entries.len()], false);
        };
        let cached: Vec<Option<Option<u64>>> = entries
            .iter()
            .map(|e| {
                self.entries
                    .get(&entry_key(archive, e))
                    .filter(|x| x.1 == s && x.2 == t)
                    .map(|x| x.0)
            })
            .collect();
        if cached.iter().all(Option::is_some) {
            return (cached.into_iter().map(Option::flatten).collect(), false);
        }
        let got = read_archive_hashes(archive, entries);
        for (e, h) in entries.iter().zip(&got) {
            self.entries.insert(entry_key(archive, e), (*h, s, t));
        }
        self.dirty = true;
        (got, true)
    }
}

/// Reads the asked entries of one archive and hashes their bytes.
fn read_archive_hashes(archive: &Path, entries: &[String]) -> Vec<Option<u64>> {
    let want: HashMap<&str, usize> = entries.iter().enumerate().map(|(i, e)| (e.as_str(), i)).collect();
    let mut out = vec![None; entries.len()];
    if let Ok(f) = std::fs::File::open(archive) {
        let _ = sat_formats::archive_data::read_entries(
            std::io::BufReader::new(f),
            |p| want.contains_key(p),
            |p, bytes| {
                if let (Some(&i), Some(b)) = (want.get(p), bytes) {
                    out[i] = Some(xxhash_rust::xxh3::xxh3_64(&b));
                }
            },
        );
    }
    out
}

// @sat:scan.content-hash.table
/// Content hashes for a provider table (`@sat:identical.needs` decides which loose copies are read; the rest stay
/// unknown): `folder(mod name)` — where the mod's files are. Hashes are taken from and kept in `cache_file`.
/// Returns the hashes and how many files (loose files and archives) were read now.
pub fn hashes_for(
    table: &sat_core::table::ProviderTable,
    folder: impl Fn(&str) -> PathBuf,
    cache_file: &Path,
    min_size: u64,
) -> (sat_core::identical::ContentHashes, usize) {
    use sat_core::table::Source;
    let need = sat_core::identical::needs_hash(table, min_size);
    let names = table.mod_names();
    let (loose, packed): (Vec<usize>, Vec<usize>) =
        need.into_iter().partition(|&i| table.source(i) == Source::Loose);
    let paths: Vec<PathBuf> = loose
        .iter()
        .map(|&i| folder(&names[table.owner(i).idx()]).join(table.path_string(table.path_of_provider(i))))
        .collect();
    let mut cache = HashCache::load(cache_file);
    let (hashes, mut read) = cache.hash_all(&paths);
    let mut out = vec![None; table.provider_count()];
    for (i, h) in loose.into_iter().zip(hashes) {
        out[i] = h;
    }
    // archive entries, one archive at a time
    let mut by_archive: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
    for i in packed {
        if let Source::Archive(a) = table.source(i) {
            by_archive.entry(a.0).or_default().push(i);
        }
    }
    for (a, provs) in by_archive {
        let r = &table.archives()[a as usize];
        let file = folder(&names[r.owner.idx()]).join(&r.file_name);
        let entries: Vec<String> = provs
            .iter()
            .map(|&i| table.path_string(table.path_of_provider(i)))
            .collect();
        let (hs, was_read) = cache.hash_archive(&file, &entries);
        read += usize::from(was_read);
        for (i, h) in provs.into_iter().zip(hs) {
            out[i] = h;
        }
    }
    let _ = cache.save(cache_file);
    (sat_core::identical::ContentHashes(out), read)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_packed_copy_compares_with_a_loose_one() {
        use sat_core::facts::{ArchiveFacts, LooseDir, LooseFile, ModFacts};
        let d = std::env::temp_dir().join(format!("sat-content-arc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (a, b) = (d.join("A"), d.join("B"));
        std::fs::create_dir_all(a.join("textures")).unwrap();
        std::fs::create_dir_all(b.join("src")).unwrap();
        std::fs::write(a.join("textures").join("x.dds"), b"the same texture").unwrap();
        std::fs::write(b.join("src").join("x.dds"), b"the same texture").unwrap();
        std::fs::write(b.join("src").join("y.dds"), b"another").unwrap();
        let files = [("textures/x.dds", "x.dds"), ("textures/y.dds", "y.dds")].map(|(path, src)| {
            sat_formats::bsa_write::PackFile {
                path: path.into(),
                source: b.join("src").join(src),
            }
        });
        let bsa = b.join("B.bsa");
        sat_formats::bsa_write::write_bsa(std::fs::File::create(&bsa).unwrap(), &files).unwrap();
        let manifest = sat_formats::read_archive(std::fs::File::open(&bsa).unwrap()).unwrap();
        let facts = [
            ModFacts {
                name: "A".into(),
                dirs: vec![LooseDir {
                    rel: "textures".into(),
                    files: vec![
                        LooseFile {
                            name: "x.dds".into(),
                            size: 16,
                            vram: 0,
                        },
                        LooseFile {
                            name: "y.dds".into(),
                            size: 7,
                            vram: 0,
                        },
                    ],
                }],
                ..ModFacts::default()
            },
            ModFacts {
                name: "B".into(),
                archives: vec![ArchiveFacts {
                    file_name: "B.bsa".into(),
                    size: 0,
                    manifest: Some(manifest),
                    error: None,
                }],
                ..ModFacts::default()
            },
        ];
        std::fs::write(a.join("textures").join("y.dds"), b"differs").unwrap();
        let table = sat_core::table::ProviderTable::build(&facts);
        let cache = d.join("hashes.txt");
        let (h, read) = hashes_for(&table, |m| d.join(m), &cache, 0);
        assert_eq!(read, 3, "two loose files and one archive");
        let by = |path: &str, archive: bool| {
            (0..table.provider_count())
                .find(|&i| {
                    table.path_string(table.path_of_provider(i)) == path && table.is_archive(i) == archive
                })
                .unwrap()
        };
        use sat_core::identical::same;
        assert_eq!(
            same(
                &table,
                &h,
                by("textures/x.dds", false),
                by("textures/x.dds", true)
            ),
            Some(true)
        );
        assert_eq!(
            same(
                &table,
                &h,
                by("textures/y.dds", false),
                by("textures/y.dds", true)
            ),
            Some(false)
        );
        let (_, read) = hashes_for(&table, |m| d.join(m), &cache, 0);
        assert_eq!(read, 0, "the archive is not read again");
        std::fs::remove_dir_all(&d).unwrap();
    }

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
