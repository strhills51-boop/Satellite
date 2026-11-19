//! Content hashes of files, with a cache that skips files unchanged since the last time. @sat:scan.content-hash
//!
//! Owns: the hash of a file's bytes (xxh3, 64 bits), and the hash cache — a text file of `hash size mtime path`
//! lines (`x` for a hash: not comparable); a line counts while the file keeps its size and modification time.
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

/// Size and modification time (nanoseconds since 1970: a `u64` holds them until 2554).
fn stamp(path: &Path) -> Option<(u64, u64)> {
    let md = std::fs::metadata(path).ok()?;
    let t = md.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((md.len(), u64::try_from(t).ok()?))
}

// @sat:scan.content-hash.cache
/// Hashes kept between runs: files by path, archive entries as `archive::entry` with the archive's size and time;
/// an entry that cannot be compared is kept too (so the archive is not read again for it).
///
/// Millions of lines on a big build: the cache keeps a 64-bit hash of each path's text, not the path (a 4 GB machine
/// cannot hold the paths twice), and reads the file line by line. Saving streams the old file and writes the lines
/// set this run after the ones they replace are dropped.
#[derive(Debug, Default)]
pub struct HashCache {
    entries: HashMap<u64, Entry>,
    /// Entries set this run, with their path text (the save writes them).
    fresh: Vec<(String, Entry)>,
    dirty: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    /// `None` — not comparable.
    hash: Option<u64>,
    size: u64,
    time: u64,
}

fn key(path: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(path.as_bytes())
}

/// A cache line: `hash size mtime path` (`x` for a hash: not comparable); `None` — not a line of the cache (or a
/// `-` line from before entry paths were matched normalized: such an entry is read again).
fn parse_line(line: &str) -> Option<(&str, Entry)> {
    let mut it = line.splitn(4, ' ');
    let (h, s, t, p) = (it.next()?, it.next()?, it.next()?, it.next()?);
    let hash = match h {
        "x" => None,
        "-" => return None,
        _ => Some(u64::from_str_radix(h, 16).ok()?),
    };
    Some((
        p,
        Entry {
            hash,
            size: s.parse().ok()?,
            time: t.parse().ok()?,
        },
    ))
}

fn write_line(out: &mut impl std::io::Write, p: &str, e: &Entry) -> std::io::Result<()> {
    match e.hash {
        Some(h) => writeln!(out, "{h:016x} {} {} {p}", e.size, e.time),
        None => writeln!(out, "x {} {} {p}", e.size, e.time),
    }
}

impl HashCache {
    pub fn load(file: &Path) -> Self {
        use std::io::BufRead;
        let mut entries = HashMap::new();
        if let Ok(f) = std::fs::File::open(file) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Some((p, e)) = parse_line(&line) {
                    entries.insert(key(p), e);
                }
            }
        }
        Self {
            entries,
            fresh: Vec::new(),
            dirty: false,
        }
    }

    fn get(&self, path: &str, size: u64, time: u64) -> Option<Option<u64>> {
        self.entries
            .get(&key(path))
            .filter(|e| e.size == size && e.time == time)
            .map(|e| e.hash)
    }

    fn set(&mut self, path: String, e: Entry) {
        self.entries.insert(key(&path), e);
        self.fresh.push((path, e));
        self.dirty = true;
    }

    /// Writes the cache: the lines of the old file that this run did not replace, then this run's entries.
    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        use std::io::{BufRead, Write};
        if !self.dirty {
            return Ok(());
        }
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let replaced: std::collections::HashSet<u64> = self.fresh.iter().map(|(p, _)| key(p)).collect();
        let tmp = file.with_extension("tmp");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        if let Ok(f) = std::fs::File::open(file) {
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Some((p, _)) = parse_line(&line)
                    && !replaced.contains(&key(p))
                {
                    writeln!(out, "{line}")?;
                }
            }
        }
        // the last entry set for a path wins
        let mut written = std::collections::HashSet::new();
        for (p, e) in self.fresh.iter().rev() {
            if written.insert(key(p)) {
                write_line(&mut out, p, e)?;
            }
        }
        out.flush()?;
        drop(out);
        std::fs::rename(tmp, file)
    }

    /// The hash recorded for a file while its size and time are the ones recorded (`None` — no such record).
    pub fn recorded(&self, path: &Path) -> Option<u64> {
        let (s, t) = stamp(path)?;
        self.get(&path.to_string_lossy(), s, t).flatten()
    }

    /// Hashes of `paths` (in their order; `None` — not readable), from the cache where the file is unchanged, else
    /// read in parallel. Returns how many were read.
    pub fn hash_all(&mut self, paths: &[PathBuf]) -> (Vec<Option<u64>>, usize) {
        let stamps: Vec<Option<(u64, u64)>> = paths.par_iter().map(|p| stamp(p)).collect();
        let fresh: Vec<Option<u64>> = paths
            .iter()
            .zip(&stamps)
            .map(|(p, st)| {
                let (s, t) = (*st)?;
                self.get(&p.to_string_lossy(), s, t).flatten()
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
            if let (Some(h), Some((size, time))) = (h, stamps[*i]) {
                self.set(
                    paths[*i].to_string_lossy().into_owned(),
                    Entry {
                        hash: Some(*h),
                        size,
                        time,
                    },
                );
            }
        }
        (out, read.len())
    }
}

fn entry_key(archive: &Path, entry: &str) -> String {
    format!("{}::{entry}", archive.display())
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
            .map(|e| self.get(&entry_key(archive, e), s, t))
            .collect();
        if cached.iter().all(Option::is_some) {
            return (cached.into_iter().map(Option::flatten).collect(), false);
        }
        let got = read_archive_hashes(archive, entries);
        for (e, h) in entries.iter().zip(&got) {
            self.set(
                entry_key(archive, e),
                Entry {
                    hash: *h,
                    size: s,
                    time: t,
                },
            );
        }
        (got, true)
    }
}

/// Reads the asked entries of one archive and hashes their bytes.
fn read_archive_hashes(archive: &Path, entries: &[String]) -> Vec<Option<u64>> {
    // the reader gives entry paths normalized (lower case, forward slashes); the table keeps the author's case
    let want: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (sat_formats::normalize_path(e), i))
        .collect();
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
        // the loose copy is written in the author's case: the table keeps it, the archive reader normalizes
        std::fs::create_dir_all(a.join("Textures")).unwrap();
        std::fs::create_dir_all(b.join("src")).unwrap();
        std::fs::write(a.join("Textures").join("x.dds"), b"the same texture").unwrap();
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
                    rel: "Textures".into(),
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
        std::fs::write(a.join("Textures").join("y.dds"), b"differs").unwrap();
        let table = sat_core::table::ProviderTable::build(&facts);
        let cache = d.join("hashes.txt");
        let (h, read) = hashes_for(&table, |m| d.join(m), &cache, 0);
        assert_eq!(read, 3, "two loose files and one archive");
        let by = |path: &str, archive: bool| {
            (0..table.provider_count())
                .find(|&i| {
                    table
                        .path_string(table.path_of_provider(i))
                        .eq_ignore_ascii_case(path)
                        && table.is_archive(i) == archive
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
        // the save keeps the old lines it did not replace and writes the new one once
        again.save(&file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text.lines().count(), 3, "{text}");
        let mut third = HashCache::load(&file);
        assert_eq!(third.hash_all(&paths).1, 0, "all three come back from the file");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
