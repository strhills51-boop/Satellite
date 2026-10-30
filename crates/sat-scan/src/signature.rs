//! Cheap change signature of a mod folder.

use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::{CACHE_VERSION, ScanOptions};

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

/// Names, sizes and modification times of entries up to depth 2, plus options that change the result.
///
/// Installing/updating a mod through a manager replaces files, which changes mtimes of the touched
/// top-level directories. Edits deep inside an otherwise untouched tree are not detected — the run
/// offers a forced rescan for that (`ScanOptions::force`).
pub fn signature(dir: &Path, opts: &ScanOptions) -> u64 {
    let mut h = Fnv::new();
    h.write_u64(u64::from(CACHE_VERSION));
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
        // Not `DirEntry::metadata()`: on Windows it comes from the parent directory's index, which NTFS updates
        // lazily, so the same untouched folder could yield two different signatures. A handle-based query is exact;
        // the cost is one extra call per entry, only down to depth 2.
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
            visit(&e.path(), depth + 1, h);
        }
    }
}
