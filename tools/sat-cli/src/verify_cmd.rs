//! `sat-cli verify-formats`: the format readers over a real folder of mods, with their invariants. @sat:cli.verify-formats
//!
//! Owns: walking a folder, reading every BSA/BA2, plugin and DDS in it with the readers of `sat-formats`, checking
//! what must hold for a file that was read right, and a report — versions met, problems by kind with paths.
//! Not here: the readers (`@sat:formats`), the scan of a build (`@sat:scan`).
//!
//! The format tests on real samples (PHILOSOPHY, phase 0): mod files cannot go into the repository, so the corpus
//! is the user's own build. Unit tests cover the readers on made files; this covers them on what is out there.

use std::collections::BTreeMap;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use sat_formats::{ArchiveKind, DdsInfo, read_archive, read_plugin_header, scan_records};

use crate::{flag_value, has};

#[derive(Default)]
struct Report {
    /// What was read, by format and version (`bsa v105`, `plugin 1.71`, `dds BC7_UNORM`).
    seen: BTreeMap<String, usize>,
    /// Problems by kind, with the first paths.
    problems: BTreeMap<String, (usize, Vec<String>)>,
    files: usize,
}

impl Report {
    fn saw(&mut self, what: String) {
        *self.seen.entry(what).or_default() += 1;
    }
    fn problem(&mut self, kind: &str, path: &Path, detail: String) {
        let e = self.problems.entry(kind.to_string()).or_default();
        e.0 += 1;
        if e.1.len() < 200 {
            e.1.push(format!("{}  {detail}", path.display()));
        }
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => walk(&p, out),
            Ok(t) if t.is_file() => {
                let ext = p
                    .extension()
                    .map(|x| x.to_string_lossy().to_lowercase())
                    .unwrap_or_default();
                if matches!(ext.as_str(), "bsa" | "ba2" | "esp" | "esm" | "esl" | "dds") {
                    out.push(p);
                }
            }
            _ => {}
        }
    }
}

// @sat:cli.verify-formats.archive
fn archive(p: &Path, r: &mut Report) {
    let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let m = match std::fs::File::open(p)
        .map_err(|e| e.to_string())
        .and_then(|f| read_archive(BufReader::new(f)).map_err(|e| e.to_string()))
    {
        Ok(m) => m,
        Err(e) => return r.problem("archive: not read", p, e),
    };
    r.saw(match m.kind {
        ArchiveKind::Bsa { version } => format!("bsa v{version}"),
        ArchiveKind::Ba2General { version } => format!("ba2 GNRL v{version}"),
        ArchiveKind::Ba2Textures { version } => format!("ba2 DX10 v{version}"),
    });
    if m.entries.is_empty() {
        r.problem("archive: empty", p, String::new());
    }
    let mut seen = std::collections::HashSet::new();
    for e in &m.entries {
        if e.path.is_empty()
            || e.path.starts_with('/')
            || e.path.contains('\\')
            || e.path != e.path.to_lowercase()
        {
            r.problem("archive: path not normalized", p, e.path.clone());
            break;
        }
        if !seen.insert(e.path.as_str()) {
            r.problem("archive: duplicate path", p, e.path.clone());
            break;
        }
    }
    let stored: u64 = m.entries.iter().map(|e| e.size).sum();
    // unpacked sizes may exceed the file; stored ones may not, and nothing packs 64 times over
    if size > 0 && stored > size.saturating_mul(64) {
        r.problem(
            "archive: sizes beyond the file",
            p,
            format!("{stored} in {size} bytes"),
        );
    }
    if let ArchiveKind::Ba2Textures { .. } = m.kind
        && let Some(e) = m.entries.iter().find(|e| e.vram == 0)
    {
        r.problem("archive: DX10 texture without size", p, e.path.clone());
    }
}

// @sat:cli.verify-formats.plugin
fn plugin(p: &Path, r: &mut Report) {
    let open = || std::fs::File::open(p).map_err(|e| e.to_string());
    let h = match open().and_then(|f| read_plugin_header(BufReader::new(f)).map_err(|e| e.to_string())) {
        Ok(h) => h,
        Err(e) => return r.problem("plugin: header not read", p, e),
    };
    r.saw(format!("plugin header {:.2}", h.version));
    if !(0.9..=2.0).contains(&h.version) {
        r.problem("plugin: odd header version", p, format!("{}", h.version));
    }
    if let Some(m) = h.masters.iter().find(|m| {
        let l = m.to_ascii_lowercase();
        !(l.ends_with(".esm") || l.ends_with(".esp") || l.ends_with(".esl"))
    }) {
        r.problem("plugin: odd master name", p, m.clone());
    }
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if h.masters.iter().any(|m| m.to_lowercase() == name) {
        r.problem("plugin: its own master", p, name.clone());
    }
    match open().and_then(|f| scan_records(BufReader::with_capacity(1 << 20, f)).map_err(|e| e.to_string())) {
        Ok(c) if c.total == 0 && std::fs::metadata(p).map(|m| m.len()).unwrap_or(0) > 1024 => {
            r.problem("plugin: no records in a large file", p, String::new())
        }
        Ok(_) => {}
        Err(e) => r.problem("plugin: records not walked", p, e),
    }
}

// @sat:cli.verify-formats.dds
fn dds(p: &Path, r: &mut Report) {
    let mut head = [0u8; 148];
    let n = match std::fs::File::open(p).and_then(|mut f| f.read(&mut head)) {
        Ok(n) => n,
        Err(e) => return r.problem("dds: not read", p, e.to_string()),
    };
    let Some(d) = DdsInfo::parse(&head[..n]) else {
        return r.problem("dds: not a DDS header", p, String::new());
    };
    r.saw(format!("dds {}", d.format));
    let largest = d.width.max(d.height).max(1);
    let full_chain = 32 - largest.leading_zeros();
    if d.width == 0 || d.height == 0 {
        r.problem("dds: zero size", p, format!("{}x{}", d.width, d.height));
    } else if d.mips > full_chain {
        r.problem(
            "dds: more mips than the size has",
            p,
            format!("{} for {}x{}", d.mips, d.width, d.height),
        );
    }
    if largest > 16384 {
        r.problem("dds: larger than 16384", p, format!("{}x{}", d.width, d.height));
    }
    if !d.known() {
        r.problem("dds: GPU size unknown", p, d.format.clone());
    }
}

/// `sat-cli verify-formats <dir> [--dds N]`: reads every archive and plugin under `dir` and up to `N` DDS files
/// (default all); prints what was met and the problems. Fails when a file could not be read at all.
pub fn verify(args: &[String]) -> Result<(), String> {
    let dir = args
        .iter()
        .find(|a| {
            !a.starts_with("--")
                && Some(a.as_str()) != flag_value(args, "--dds")
                && Some(a.as_str()) != flag_value(args, "--out")
        })
        .ok_or("usage: sat-cli verify-formats <dir> [--dds N] [--out FILE]")?;
    let t = std::time::Instant::now();
    let mut files = Vec::new();
    walk(Path::new(dir), &mut files);
    files.sort();
    let dds_limit: usize = flag_value(args, "--dds")
        .and_then(|n| n.parse().ok())
        .unwrap_or(usize::MAX);
    let mut dds_taken = 0;
    files.retain(|p| {
        let is_dds = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds"));
        if is_dds {
            dds_taken += 1;
        }
        !is_dds || dds_taken <= dds_limit
    });
    let report = Mutex::new(Report::default());
    files.par_iter().for_each(|p| {
        let mut r = Report::default();
        match p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .as_deref()
        {
            Some("bsa" | "ba2") => archive(p, &mut r),
            Some("dds") => dds(p, &mut r),
            _ => plugin(p, &mut r),
        }
        let mut all = report.lock().expect("report");
        all.files += 1;
        for (k, n) in r.seen {
            *all.seen.entry(k).or_default() += n;
        }
        for (k, (n, paths)) in r.problems {
            let e = all.problems.entry(k).or_default();
            e.0 += n;
            for x in paths {
                if e.1.len() < 200 {
                    e.1.push(x);
                }
            }
        }
    });
    let r = report.into_inner().expect("report");
    println!(
        "read {} file(s) under {dir} in {:.1} s",
        r.files,
        t.elapsed().as_secs_f64()
    );
    let mut seen: Vec<_> = r.seen.iter().collect();
    seen.sort_by_key(|x| std::cmp::Reverse(*x.1));
    for (k, n) in seen.iter().take(if has(args, "--all") { usize::MAX } else { 40 }) {
        println!("  {n:>8}  {k}");
    }
    if r.problems.is_empty() {
        println!("no problems");
        return Ok(());
    }
    // on screen: kinds and a few paths; `--out FILE` keeps up to 200 paths of each kind
    println!("problems:");
    for (k, (n, paths)) in &r.problems {
        println!("  {n:>8}  {k}");
        for p in paths.iter().take(3) {
            println!("            {p}");
        }
    }
    if let Some(out) = flag_value(args, "--out") {
        let mut text = String::new();
        for (k, (n, paths)) in &r.problems {
            text.push_str(&format!("{n}  {k}\n"));
            for p in paths {
                text.push_str(&format!("    {p}\n"));
            }
        }
        std::fs::write(out, text).map_err(|e| format!("{out}: {e}"))?;
        println!("full list → {out}");
    }
    let unread: usize = r
        .problems
        .iter()
        .filter(|(k, _)| {
            k.ends_with("not read") || k.ends_with("not walked") || k.ends_with("not a DDS header")
        })
        .map(|(_, (n, _))| n)
        .sum();
    if unread > 0 {
        return Err(format!("{unread} file(s) could not be read"));
    }
    Ok(())
}
