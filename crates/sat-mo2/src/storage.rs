//! Storage on disk: what a file is and what Satellite does to it, with a journal. @sat:mo2.storage
//!
//! What a file is: compressed or not, one file with another path or not. What Satellite does: a link in place of an
//! identical copy, WOF compression and back.
//!
//! Owns: the storage facts read from the disk (a WOF-compressed file takes less on disk than its size — the WOF
//! filter hides its reparse point from every listing and query, so the space it takes is the one sign; two paths
//! are one file when the volume and file index match — `same-file`), replacing a copy by a hard link to its keeper (same size, same
//! bytes checked byte for byte, an atomic rename over the copy), making a linked copy a file of its own again,
//! compressing and decompressing through Windows' own `compact.exe /exe` in batches, and the journal of all of it
//! (`<profile>/satellite/storage-journal.tsv`).
//! Not here: what should happen (`@sat:storage`), the command (`@sat:cli.store`).
//!
//! ADR 0022. No `unsafe` here: links are `std::fs::hard_link`, WOF goes through the system tool, the space on disk
//! comes from `filesize` (`GetCompressedFileSizeW`). The disk is the truth; the journal only says what Satellite did
//! and how to undo it.

use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use sat_core::storage::Codec;

// @sat:mo2.storage.facts
/// What a file takes on disk (`None` — not readable). On Windows the compressed size; elsewhere the allocated blocks.
pub fn on_disk(path: &Path) -> Option<u64> {
    filesize::file_real_size(path).ok()
}

/// Whether a file of `size` bytes is compressed: it takes less on disk than its size. (Checked on Windows on the
/// owner's disk 2026-11-19: a WOF file shows no reparse point, no attribute — only its size on disk is smaller.)
pub fn compressed_at(path: &Path, size: u64) -> bool {
    on_disk(path).is_some_and(|d| d < size)
}

/// Whether a file is compressed (its size read from the disk).
pub fn is_compressed(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && compressed_at(path, m.len()))
}

/// The file a path is on disk, as a number: equal numbers — one file (hard links); `0` — not readable.
pub fn file_id(path: &Path) -> u128 {
    use std::hash::{Hash, Hasher};
    match same_file::Handle::from_path(path) {
        // the handle hashes the volume and the file index; a clash between a few copies only skips a link
        Ok(h) => {
            let mut s = std::collections::hash_map::DefaultHasher::new();
            h.hash(&mut s);
            u128::from(s.finish()) | (1u128 << 64)
        }
        Err(_) => 0,
    }
}

/// Two files have the same bytes, read side by side.
pub fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    let (ma, mb) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (std::fs::File::open(a)?, std::fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 1 << 20], vec![0u8; 1 << 20]);
    loop {
        let na = read_full(&mut fa, &mut ba)?;
        let nb = read_full(&mut fb, &mut bb)?;
        if na != nb || ba[..na] != bb[..nb] {
            return Ok(false);
        }
        if na == 0 {
            return Ok(true);
        }
    }
}

fn read_full(f: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        let k = f.read(&mut buf[n..])?;
        if k == 0 {
            break;
        }
        n += k;
    }
    Ok(n)
}

fn temp_beside(target: &Path, tag: &str) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!(".{name}.{tag}"))
}

/// Replace `tmp` over `target` (a read-only target is made writable first: Windows does not rename over one).
fn replace(tmp: &Path, target: &Path) -> Result<(), String> {
    let _ = set_locked(target, false);
    std::fs::rename(tmp, target).map_err(|e| {
        let _ = std::fs::remove_file(tmp);
        format!("{}: {e}", target.display())
    })
}

// @sat:mo2.storage.link
/// Replace `copy` by a hard link to `keeper`: only when both are the same size and the same bytes; the link is made
/// under a temporary name beside the copy and renamed over it, so the copy is never missing.
pub fn link_copy(keeper: &Path, copy: &Path) -> Result<(), String> {
    match same_bytes(keeper, copy) {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "{}: the bytes differ from {}",
                copy.display(),
                keeper.display()
            ));
        }
        Err(e) => return Err(format!("{}: {e}", copy.display())),
    }
    let tmp = temp_beside(copy, "satlink");
    let _ = std::fs::remove_file(&tmp);
    std::fs::hard_link(keeper, &tmp).map_err(|e| format!("{}: {e}", copy.display()))?;
    replace(&tmp, copy)
}

/// Make a linked name a file of its own again (its bytes copied out of the shared file); the new file carries no
/// lock. Replacing the name lifts the shared file's lock for its other names too, so a `partner` — another name of
/// the shared file — is locked again when it was locked.
pub fn unlink_copy(copy: &Path, partner: Option<&Path>) -> Result<(), String> {
    let locked = is_locked(copy);
    let tmp = temp_beside(copy, "satunlink");
    let _ = std::fs::remove_file(&tmp);
    std::fs::copy(copy, &tmp).map_err(|e| format!("{}: {e}", copy.display()))?;
    let _ = set_locked(&tmp, false);
    replace(&tmp, copy)?;
    if let (true, Some(p)) = (locked, partner) {
        let _ = set_locked(p, true);
    }
    Ok(())
}

// @sat:mo2.storage.lock
/// The lock on a shared file: Windows' read-only attribute. It belongs to the file, not to a name, so every name of
/// a linked file is locked at once: an editor that would save over it in place fails loudly instead of changing the
/// copies in other mods. MO2 installs, merges and removes over it (checked on the owner's MO2 2026-11-19: a merge
/// over a linked mod replaced its files — the copy kept its bytes — and a merge and a removal over a locked one went
/// through); `compact.exe` compresses and decompresses it.
pub fn set_locked(path: &Path, on: bool) -> std::io::Result<()> {
    let mut p = std::fs::metadata(path)?.permissions();
    if p.readonly() == on {
        return Ok(());
    }
    #[cfg(unix)]
    {
        // elsewhere the owner's write bit (tests): never everyone's
        use std::os::unix::fs::PermissionsExt;
        p.set_mode(if on { p.mode() & !0o222 } else { p.mode() | 0o200 });
    }
    #[cfg(not(unix))]
    {
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(on);
    }
    std::fs::set_permissions(path, p)
}

/// Whether a file is locked (read from the file itself: a folder listing may show another name's stale attributes).
pub fn is_locked(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.permissions().readonly())
}

// @sat:mo2.storage.compact
/// Compress (`Some(codec)`) or decompress (`None`) files with `compact.exe /exe`, in batches that keep the command
/// line short. Returns each file and whether it ended up as asked (read back from the disk: compressed — it takes
/// less than its size; a file compression does not shrink stays as it was, and WOF leaves it so).
pub fn compact(files: &[PathBuf], codec: Option<Codec>) -> Result<Vec<(PathBuf, bool)>, String> {
    if !cfg!(windows) {
        return Err("WOF compression is Windows-only (compact.exe)".into());
    }
    let mut out = Vec::with_capacity(files.len());
    let mut batch: Vec<&PathBuf> = Vec::new();
    let mut len = 0usize;
    let run = |batch: &[&PathBuf], out: &mut Vec<(PathBuf, bool)>| -> Result<(), String> {
        if batch.is_empty() {
            return Ok(());
        }
        let mut c = Command::new("compact");
        match codec {
            Some(k) => c.arg("/c").arg(format!("/exe:{}", k.compact_name())),
            None => c.arg("/u").arg("/exe"),
        };
        c.args(["/i", "/q"]).args(batch.iter().map(|p| p.as_os_str()));
        let status = c.output().map_err(|e| format!("compact: {e}"))?;
        let _ = status;
        for p in batch {
            let now = is_compressed(p);
            out.push(((*p).clone(), now == codec.is_some()));
        }
        Ok(())
    };
    for f in files {
        let l = f.as_os_str().len() + 3;
        if !batch.is_empty() && len + l > 24_000 {
            run(&batch, &mut out)?;
            batch.clear();
            len = 0;
        }
        batch.push(f);
        len += l;
    }
    run(&batch, &mut out)?;
    Ok(out)
}

// @sat:mo2.storage.journal
/// One thing Satellite did to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalLine {
    /// Seconds since the Unix epoch.
    pub time: u64,
    /// `link`, `unlink`, `compress`, `decompress`.
    pub op: String,
    /// The codec of a compression; empty otherwise.
    pub codec: String,
    pub path: String,
    /// The keeper a link points to; empty otherwise.
    pub keeper: String,
    pub size: u64,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The journal open for appending: each line goes to the disk in one write as soon as it is made, so a laptop that
/// dies mid-run loses at most the step in hand.
#[derive(Debug)]
pub struct Journal {
    file: std::fs::File,
}

impl Journal {
    /// Open (or create, with the header) the journal.
    pub fn open(file: &Path) -> std::io::Result<Self> {
        if let Some(d) = file.parent() {
            std::fs::create_dir_all(d)?;
        }
        let new = !file.exists();
        let mut f = std::fs::OpenOptions::new().create(true).append(true).open(file)?;
        if new {
            f.write_all(b"time\top\tcodec\tpath\tkeeper\tsize\n")?;
        }
        Ok(Self { file: f })
    }

    pub fn write(&mut self, l: &JournalLine) -> std::io::Result<()> {
        let line = format!(
            "{}\t{}\t{}\t{}\t{}\t{}\n",
            l.time, l.op, l.codec, l.path, l.keeper, l.size
        );
        self.file.write_all(line.as_bytes())
    }
}

/// Append lines to the journal (created with a header when new).
pub fn journal_append(file: &Path, lines: &[JournalLine]) -> std::io::Result<()> {
    if lines.is_empty() {
        return Ok(());
    }
    let mut j = Journal::open(file)?;
    lines.iter().try_for_each(|l| j.write(l))
}

/// The WOF-compressed files under `dir` (the disk's answer to "what is compressed here", journal or not).
pub fn compressed_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push(p),
                Ok(t) if t.is_file() && is_compressed(&p) => out.push(p),
                _ => {}
            }
        }
    }
    out
}

/// The journal, oldest first (a missing file is an empty journal).
pub fn journal_read(file: &Path) -> Vec<JournalLine> {
    let Ok(f) = std::fs::File::open(file) else {
        return Vec::new();
    };
    std::io::BufReader::new(f)
        .lines()
        .skip(1)
        .map_while(Result::ok)
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (c.len() == 6).then(|| JournalLine {
                time: c[0].parse().unwrap_or(0),
                op: c[1].to_string(),
                codec: c[2].to_string(),
                path: c[3].to_string(),
                keeper: c[4].to_string(),
                size: c[5].parse().unwrap_or(0),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_becomes_a_link_only_with_the_same_bytes_and_comes_back() {
        let d = tempfile::tempdir().unwrap();
        let (k, c, x) = (
            d.path().join("k.dds"),
            d.path().join("c.dds"),
            d.path().join("x.dds"),
        );
        std::fs::write(&k, b"same bytes").unwrap();
        std::fs::write(&c, b"same bytes").unwrap();
        std::fs::write(&x, b"other byte").unwrap();
        assert_ne!(file_id(&k), file_id(&c));
        assert!(link_copy(&k, &x).is_err(), "different bytes are never linked");
        link_copy(&k, &c).unwrap();
        assert_eq!(file_id(&k), file_id(&c), "one file now");
        assert_eq!(std::fs::read(&c).unwrap(), b"same bytes");
        assert!(!temp_beside(&c, "satlink").exists());
        set_locked(&c, true).unwrap();
        assert!(
            is_locked(&k),
            "the lock belongs to the file: every name is locked"
        );
        unlink_copy(&c, Some(&k)).unwrap();
        assert_ne!(file_id(&k), file_id(&c), "a file of its own again");
        assert!(!is_locked(&c), "a file of its own carries no lock");
        assert!(is_locked(&k), "the partner is locked again");
        set_locked(&k, false).unwrap();
        assert_eq!(std::fs::read(&c).unwrap(), b"same bytes");
        assert_eq!(file_id(&d.path().join("none")), 0);
        assert!(!is_compressed(&k), "a plain file takes at least its size");
        assert!(!compressed_at(&d.path().join("none"), 10));
    }

    #[test]
    fn the_journal_reads_back_what_was_written() {
        let d = tempfile::tempdir().unwrap();
        let j = d.path().join("satellite").join("storage-journal.tsv");
        let line = JournalLine {
            time: 1,
            op: "compress".into(),
            codec: "LZX".into(),
            path: "A/textures/x.dds".into(),
            keeper: String::new(),
            size: 42,
        };
        journal_append(&j, std::slice::from_ref(&line)).unwrap();
        journal_append(&j, std::slice::from_ref(&line)).unwrap();
        assert_eq!(journal_read(&j), [line.clone(), line]);
        assert!(journal_read(&d.path().join("none")).is_empty());
    }
}
