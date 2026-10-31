//! Cheap change signature of a mod folder. @sat:scan.signature
//!
//! Owns: content signatures of mod folders (v3, legacy v2/v1 for cache adoption).

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::ScanOptions;

/// The global cache version of the first cache format; legacy signatures hashed it.
const LEGACY_CACHE_VERSION: u32 = 1;

/// Stable FNV-1a (must not change between builds: signatures are persisted).
pub(crate) struct Fnv(u64);

impl Fnv {
    pub(crate) fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    pub(crate) fn write(&mut self, b: &[u8]) {
        for &c in b {
            self.0 ^= u64::from(c);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    pub(crate) fn write_u64(&mut self, v: u64) {
        self.write(&v.to_le_bytes());
    }
    pub(crate) fn finish(&self) -> u64 {
        self.0
    }
}

// @sat:scan.signature.v3
/// Names of entries up to depth 2, sizes and modification times of files among them. Content only: what the
/// analyzers did with it is recorded by their stamps in the cache ([`crate::analyzer::Stamp`]).
///
/// Installing/updating a mod through a manager replaces files, which changes mtimes of the touched
/// top-level directories. Edits deep inside an otherwise untouched tree are not detected — the run
/// offers a forced rescan for that (`ScanOptions::force`).
pub fn signature(dir: &Path) -> u64 {
    let mut h = Fnv::new();
    h.write(b"sat-signature-v3");
    visit(dir, 0, &mut h);
    h.finish()
}

/// The second signature: the same listing plus the options of the first cache format. Kept only to adopt entries
/// of that format (no cold rescan after an update). Remove once caches have migrated.
pub(crate) fn signature_v2(dir: &Path, opts: &ScanOptions) -> u64 {
    let mut h = Fnv::new();
    h.write_u64(u64::from(LEGACY_CACHE_VERSION));
    for flag in [
        opts.read_dds_headers,
        opts.archives,
        opts.plugins,
        opts.record_scan,
    ] {
        h.write(&[u8::from(flag)]);
    }
    for name in &opts.ignore_root_files {
        h.write(name.as_bytes());
        h.write(b"|");
    }
    visit(dir, 0, &mut h);
    h.finish()
}

fn visit(dir: &Path, depth: u32, h: &mut Fnv) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut ents: Vec<_> = rd.filter_map(Result::ok).collect();
    ents.sort_by_key(|e| e.file_name());
    for e in ents {
        // Directory-entry metadata only: on Windows it comes with the listing (no file is opened), which is what
        // makes a warm start cheap on 7 500 mods. Two caveats shape what goes into the hash:
        // - directories: NTFS updates their timestamps in the parent index lazily, so the same untouched folder
        //   could hash differently between runs → for directories only the name is hashed; their contents are
        //   visited instead;
        // - files: size and mtime in the index are refreshed when the writer closes the file, which is the case
        //   for installed mods.
        let Ok(ft) = e.file_type() else { continue };
        h.write_u64(u64::from(depth));
        h.write(e.file_name().to_string_lossy().as_bytes());
        if ft.is_dir() {
            h.write(b"/");
            if depth < 2 {
                visit(&e.path(), depth + 1, h);
            }
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        h.write_u64(md.len());
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        h.write_u64(mtime);
    }
}

/// The first signature (handle-based metadata of every entry to depth 2). Kept only to adopt cache entries written
/// by builds that used it, so updating does not force a cold rescan. Remove once caches have migrated.
pub(crate) fn signature_v1(dir: &Path, opts: &ScanOptions) -> u64 {
    let mut h = Fnv::new();
    h.write_u64(u64::from(LEGACY_CACHE_VERSION));
    for flag in [
        opts.read_dds_headers,
        opts.archives,
        opts.plugins,
        opts.record_scan,
    ] {
        h.write(&[u8::from(flag)]);
    }
    for name in &opts.ignore_root_files {
        h.write(name.as_bytes());
        h.write(b"|");
    }
    visit_v1(dir, 0, &mut h);
    h.finish()
}

fn visit_v1(dir: &Path, depth: u32, h: &mut Fnv) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut ents: Vec<_> = rd.filter_map(Result::ok).collect();
    ents.sort_by_key(|e| e.file_name());
    for e in ents {
        let Ok(md) = std::fs::metadata(e.path()) else {
            continue;
        };
        h.write_u64(u64::from(depth));
        h.write(e.file_name().to_string_lossy().as_bytes());
        h.write_u64(md.len());
        let mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos() as u64);
        h.write_u64(mtime);
        if md.is_dir() && depth < 2 {
            visit_v1(&e.path(), depth + 1, h);
        }
    }
}
