//! Directory walk of one mod, parallel across subdirectories.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rayon::prelude::*;

use sat_core::facts::{ArchiveFacts, LooseDir, LooseFile, ModFacts, ModMetrics, PluginFacts};
use sat_formats::{DdsInfo, dds::DDS_HEADER_PROBE};

use crate::ScanOptions;

const NESTED: &[&str] = &["zip", "7z", "rar", "tar", "gz"];
const SOURCES: &[&str] = &["psc", "psd", "xcf", "blend", "max", "fbx", "obj", "spp", "sbsar"];
/// Subdirectories walked in parallel only near the root; deeper levels are cheap and stay sequential.
const PARALLEL_DEPTH: u32 = 3;

/// Partial result of one subtree; merged in order.
#[derive(Default)]
struct Subtree {
    dirs: Vec<LooseDir>,
    metrics: ModMetrics,
    error: Option<String>,
}

pub(crate) fn walk_mod(name: &str, root: &Path, opts: &ScanOptions, files: &AtomicU64) -> ModFacts {
    let mut facts = ModFacts {
        name: name.to_string(),
        ..Default::default()
    };
    let tree = walk_dir(root, "", 0, opts, files);
    facts.dirs = tree.dirs;
    facts.metrics = tree.metrics;
    facts.error = tree.error;

    // Root-level plugins and archives.
    if let Some(root_dir) = facts.dirs.iter().find(|d| d.rel.is_empty()) {
        for f in &root_dir.files {
            let ext = ext_of(&f.name);
            let path = root.join(&f.name);
            if matches!(ext.as_str(), "esp" | "esm" | "esl") {
                facts.metrics.plugins += 1;
                if opts.plugins {
                    facts.plugins.push(read_plugin(&path, &f.name, opts.record_scan));
                }
            }
            if matches!(ext.as_str(), "bsa" | "ba2") {
                facts.metrics.archives += 1;
                if opts.archives {
                    let a = read_archive(&path, &f.name, f.size);
                    if let Some(m) = &a.manifest {
                        facts.metrics.archive_files += m.entries.len() as u32;
                        for e in m.entries.iter().filter(|e| e.vram > 0) {
                            facts.metrics.vram_bytes += e.vram;
                            facts.metrics.dds_known += 1;
                        }
                    }
                    facts.archives.push(a);
                }
            }
        }
    }
    facts
}

fn walk_dir(abs: &Path, rel: &str, depth: u32, opts: &ScanOptions, counter: &AtomicU64) -> Subtree {
    let mut out = Subtree::default();
    let rd = match std::fs::read_dir(abs) {
        Ok(rd) => rd,
        Err(e) => {
            out.error = Some(format!("{}: {e}", if rel.is_empty() { "." } else { rel }));
            return out;
        }
    };
    let mut ents: Vec<_> = rd.filter_map(Result::ok).collect();
    ents.sort_by_key(|e| e.file_name());

    let mut here = LooseDir {
        rel: rel.to_string(),
        files: Vec::new(),
    };
    let mut subdirs = Vec::new();
    let m = &mut out.metrics;
    for e in ents {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            subdirs.push(name);
            continue;
        }
        let lower = name.to_lowercase();
        if depth == 0 && opts.ignore_root_files.contains(&lower) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let ext = ext_of(&lower);
        let mut file = LooseFile {
            name,
            size: md.len(),
            vram: 0,
        };
        m.files += 1;
        m.bytes += file.size;
        *m.by_extension.entry(ext.clone()).or_insert(0) += 1;
        if depth == 0 {
            m.root_files += 1;
        } else {
            let top = rel.split('/').next().unwrap_or(rel).to_lowercase();
            *m.by_top_dir.entry(top).or_insert(0) += 1;
        }
        m.max_depth = m.max_depth.max(depth);
        match ext.as_str() {
            "dds" => {
                m.dds += 1;
                if opts.read_dds_headers
                    && let Some(info) = read_dds(&e.path())
                    && info.known()
                {
                    file.vram = info.vram;
                    m.dds_known += 1;
                    m.vram_bytes += info.vram;
                    *m.dds_formats.entry(info.format).or_insert(0) += 1;
                }
            }
            "nif" => m.nif += 1,
            "pex" => m.pex += 1,
            "hkx" => m.hkx += 1,
            "dll" => m.dll += 1,
            x if NESTED.contains(&x) => m.nested_archives += 1,
            x if SOURCES.contains(&x) => m.source_files += 1,
            _ => {}
        }
        here.files.push(file);
    }
    counter.fetch_add(here.files.len() as u64, Ordering::Relaxed);
    m.dirs += subdirs.len() as u32;
    if !here.files.is_empty() {
        out.dirs.push(here);
    }

    let child = |name: &String| {
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        walk_dir(&abs.join(name), &r, depth + 1, opts, counter)
    };
    let children: Vec<Subtree> = if depth < PARALLEL_DEPTH && subdirs.len() > 1 {
        subdirs.par_iter().map(child).collect()
    } else {
        subdirs.iter().map(child).collect()
    };
    for c in children {
        out.dirs.extend(c.dirs);
        merge(&mut out.metrics, c.metrics);
        if out.error.is_none() {
            out.error = c.error;
        }
    }
    out
}

fn merge(a: &mut ModMetrics, b: ModMetrics) {
    a.bytes += b.bytes;
    a.files += b.files;
    a.dirs += b.dirs;
    a.root_files += b.root_files;
    a.max_depth = a.max_depth.max(b.max_depth);
    a.dds += b.dds;
    a.dds_known += b.dds_known;
    a.vram_bytes += b.vram_bytes;
    a.nif += b.nif;
    a.pex += b.pex;
    a.hkx += b.hkx;
    a.dll += b.dll;
    a.nested_archives += b.nested_archives;
    a.source_files += b.source_files;
    for (k, v) in b.by_extension {
        *a.by_extension.entry(k).or_insert(0) += v;
    }
    for (k, v) in b.by_top_dir {
        *a.by_top_dir.entry(k).or_insert(0) += v;
    }
    for (k, v) in b.dds_formats {
        *a.dds_formats.entry(k).or_insert(0) += v;
    }
}

fn ext_of(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

fn read_dds(path: &Path) -> Option<DdsInfo> {
    let mut f = File::open(path).ok()?;
    let mut buf = [0u8; DDS_HEADER_PROBE];
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(_) => return None,
        }
    }
    DdsInfo::parse(&buf[..n])
}

fn read_plugin(path: &Path, name: &str, records: bool) -> PluginFacts {
    let mut p = PluginFacts {
        file_name: name.to_string(),
        header: None,
        records: None,
        error: None,
    };
    match File::open(path)
        .map_err(sat_formats::FormatError::from)
        .and_then(|f| sat_formats::read_plugin_header(BufReader::new(f)))
    {
        Ok(h) => p.header = Some(h),
        Err(e) => {
            p.error = Some(e.to_string());
            return p;
        }
    }
    if records {
        match File::open(path)
            .map_err(sat_formats::FormatError::from)
            .and_then(|f| sat_formats::scan_records(BufReader::with_capacity(1 << 20, f)))
        {
            Ok(r) => p.records = Some(r),
            Err(e) => p.error = Some(e.to_string()),
        }
    }
    p
}

fn read_archive(path: &Path, name: &str, size: u64) -> ArchiveFacts {
    match File::open(path)
        .map_err(sat_formats::FormatError::from)
        .and_then(sat_formats::read_archive)
    {
        Ok(m) => ArchiveFacts {
            file_name: name.to_string(),
            size,
            manifest: Some(m),
            error: None,
        },
        Err(e) => ArchiveFacts {
            file_name: name.to_string(),
            size,
            manifest: None,
            error: Some(e.to_string()),
        },
    }
}
