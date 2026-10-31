//! Directory walk of one mod, parallel across subdirectories; every file becomes an observation for the
//! registered analyzers ([`crate::analyzer`]).

use std::path::Path;
use std::sync::atomic::Ordering;

use rayon::prelude::*;

use sat_core::facts::{ArchiveFacts, LooseDir, LooseFile, ModFacts, ModMetrics, PluginFacts};

use crate::analyzer::{Issues, Observation, Origin, Part, Registry, dispatch, dispatch_virtual};
use crate::{ScanOptions, ScanProgress};

/// Subdirectories walked in parallel only near the root; deeper levels are cheap and stay sequential.
const PARALLEL_DEPTH: u32 = 3;

pub(crate) struct Ctx<'a> {
    pub reg: &'a Registry,
    pub opts: &'a ScanOptions,
    pub progress: &'a ScanProgress,
}

/// Partial result of one subtree; merged in order.
#[derive(Default)]
struct Subtree {
    dirs: Vec<LooseDir>,
    metrics: ModMetrics,
    plugins: Vec<PluginFacts>,
    archives: Vec<ArchiveFacts>,
    issues: Issues,
    error: Option<String>,
}

/// Walks a mod folder and runs every analyzer.
pub(crate) fn walk_mod(name: &str, root: &Path, ctx: &Ctx<'_>) -> (ModFacts, Issues) {
    let tree = walk_dir(root, "", 0, ctx);
    let mut facts = ModFacts {
        name: name.to_string(),
        dirs: tree.dirs,
        archives: tree.archives,
        plugins: tree.plugins,
        metrics: tree.metrics,
        missing: false,
        error: tree.error,
    };
    let mut issues = tree.issues;
    dispatch_virtual(ctx.reg, None, &mut facts, ctx.progress, &mut issues);
    if facts.error.is_none() {
        facts.error = issues.fatal.take();
    }
    (facts, issues)
}

/// Re-runs some analyzers on a cached path index (route `indexed`): no directory walk.
pub(crate) fn reanalyze(
    facts: &mut ModFacts,
    root: &Path,
    rerun: &[String],
    cleared: &[String],
    ctx: &Ctx<'_>,
) -> Issues {
    let mut issues = Issues::default();
    for id in rerun.iter().chain(cleared) {
        if let Some(a) = ctx.reg.get(id) {
            a.clear(facts);
        }
    }
    {
        let ModFacts {
            dirs,
            metrics,
            plugins,
            archives,
            ..
        } = facts;
        for LooseDir { rel, files } in dirs.iter_mut() {
            let depth = if rel.is_empty() {
                0
            } else {
                rel.matches('/').count() as u32 + 1
            };
            let dir_abs = root.join(rel.as_str());
            for f in files.iter_mut() {
                let name = f.name.clone();
                let lower = name.to_lowercase();
                let path = dir_abs.join(&name);
                let o = Observation {
                    origin: Origin::Loose,
                    dir: rel,
                    name: &name,
                    ext: ext_of(&lower),
                    size: f.size,
                    depth,
                    path: Some(&path),
                    vram: 0,
                };
                let mut part = Part {
                    metrics,
                    file: Some(f),
                    plugins,
                    archives,
                };
                dispatch(ctx.reg, Some(rerun), &o, &mut part, ctx.progress, &mut issues);
            }
        }
    }
    dispatch_virtual(ctx.reg, Some(rerun), facts, ctx.progress, &mut issues);
    if facts.error.is_none() {
        facts.error = issues.fatal.take();
    }
    issues
}

fn walk_dir(abs: &Path, rel: &str, depth: u32, ctx: &Ctx<'_>) -> Subtree {
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
    for e in ents {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if ft.is_dir() {
            subdirs.push(name);
            continue;
        }
        let lower = name.to_lowercase();
        if depth == 0 && ctx.opts.ignore_root_files.contains(&lower) {
            continue;
        }
        let Ok(md) = e.metadata() else { continue };
        let path = e.path();
        let mut file = LooseFile {
            name: name.clone(),
            size: md.len(),
            vram: 0,
        };
        let o = Observation {
            origin: Origin::Loose,
            dir: rel,
            name: &name,
            ext: ext_of(&lower),
            size: file.size,
            depth,
            path: Some(&path),
            vram: 0,
        };
        let mut part = Part {
            metrics: &mut out.metrics,
            file: Some(&mut file),
            plugins: &mut out.plugins,
            archives: &mut out.archives,
        };
        dispatch(ctx.reg, None, &o, &mut part, ctx.progress, &mut out.issues);
        here.files.push(file);
    }
    ctx.progress
        .files
        .fetch_add(here.files.len() as u64, Ordering::Relaxed);
    out.metrics.dirs += subdirs.len() as u32;
    if !here.files.is_empty() {
        out.dirs.push(here);
    }

    let child = |name: &String| {
        let r = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        walk_dir(&abs.join(name), &r, depth + 1, ctx)
    };
    let children: Vec<Subtree> = if depth < PARALLEL_DEPTH && subdirs.len() > 1 {
        subdirs.par_iter().map(child).collect()
    } else {
        subdirs.iter().map(child).collect()
    };
    for c in children {
        out.dirs.extend(c.dirs);
        merge(&mut out.metrics, c.metrics);
        out.plugins.extend(c.plugins);
        out.archives.extend(c.archives);
        out.issues.merge(c.issues);
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
    a.plugins += b.plugins;
    a.archives += b.archives;
    a.archive_files += b.archive_files;
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

fn ext_of(lower: &str) -> &str {
    lower.rsplit_once('.').map_or("", |(_, e)| e)
}
