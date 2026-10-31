//! `sat-cli` — developer tool for NG-D001 evidence.
//!
//! ```text
//! sat-cli analyze <instance> | --mods DIR --profiles DIR [--overwrite DIR]  [--profile NAME] [--listed-only] [--project FILE] [--reimport] [--no-save] [--io-threads N] [--cache DIR] [--force] [--records] [--report FILE]
//! sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--profile NAME] [--report FILE]
//! sat-cli inspect <file.bsa|.ba2|.esp|.esm|.esl|.dds>...
//! sat-cli bench-synth [--mods N] [--files F] [--rounds R]
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use sat_core::canon::{EntryKind, Modlist};
use sat_core::facts::{LooseDir, LooseFile, ModFacts};
use sat_core::ids::ModId;
use sat_core::overwrite::{OverwriteProjection, Priorities};
use sat_core::project::{ManagerDiff, PROJECT_SCHEMA, Project};
use sat_core::sync::donor;
use sat_core::table::{ProviderTable, TableBuilder};
use sat_mo2::{Instance, profile};
use sat_scan::{ScanJob, ScanOptions, ScanProgress};
use serde::Serialize;

const OVERWRITE: &str = "Overwrite";
/// Provisional project file until the container format is decided (ADR 0007).
const PROJECT_FILE: &str = "project.v0.json";
/// Mods scanned per batch before their facts are folded into the table and dropped.
const SCAN_BATCH: usize = 256;

/// What the report needs from one scan outcome, kept after its facts are dropped.
struct Seen {
    name: String,
    missing: bool,
    from_cache: bool,
    route: StagedRoute,
    error: Option<String>,
    signature: u64,
    files: u32,
    archive_files: u32,
    elapsed: std::time::Duration,
}

/// Millions of small allocations (path strings, per-dir vectors) dominate table builds; the system allocator on
/// Windows is markedly slower at that than mimalloc.
mod bench_order;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("analyze") => analyze(&args[1..]),
        Some("donor") => donor(&args[1..]),
        Some("inspect") => inspect(&args[1..]),
        Some("bench-synth") => bench_synth(&args[1..]),
        Some("bench-order") => bench_order::run(&args[1..]),
        _ => Err("usage: sat-cli analyze <instance> | --mods DIR --profiles DIR [--overwrite DIR]  [--profile NAME] [--listed-only] [--project FILE] [--reimport] [--no-save] [--io-threads N] [--cache DIR] [--force] [--records] [--report FILE]\n       sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--profile NAME] [--report FILE]\n       sat-cli inspect <file>...\n       sat-cli bench-synth [--mods N] [--files F] [--rounds R]\n       sat-cli bench-order [--entries N] [--block B]".into()),
    };
    if let Err(e) = res {
        eprintln!("{e}");
        std::process::exit(2);
    }
}

fn flag_values<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.windows(2)
        .filter(|w| w[0] == name)
        .map(|w| w[1].as_str())
        .collect()
}
fn flag_value<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    flag_values(args, name).into_iter().next()
}
fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Peak resident set (Linux); `None` elsewhere.
fn peak_rss_mib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|kb| kb / 1024)
}
/// Current resident memory (all platforms).
fn rss_mib() -> Option<u64> {
    memory_stats::memory_stats().map(|m| (m.physical_mem >> 20) as u64)
}

#[derive(Serialize)]
struct RootInfo {
    path: String,
    exists: bool,
    /// Subfolders, separator folders included.
    folders: usize,
    separator_folders: usize,
}

#[derive(Serialize)]
struct InstanceInfo {
    profile: String,
    profiles: Vec<String>,
    roots: Vec<RootInfo>,
    modlist_mods: usize,
    modlist_enabled: usize,
    modlist_separators: usize,
    modlist_foreign: usize,
    /// Mod folders on disk (not separators) that the profile's modlist does not mention.
    unlisted_folders: usize,
    unlisted_sample: Vec<String>,
    /// Scanned as inactive (default; `--listed-only` skips them). See ADR 0006: the run covers every folder.
    unlisted_scanned: bool,
}

#[derive(Serialize)]
struct ScanBreakdown {
    threads: usize,
    /// Thread time summed over workers, ms. Divide by `threads` for a rough wall-time share.
    signature_ms: u64,
    cache_load_ms: u64,
    walk_ms: u64,
    dds_reads: u64,
    dds_ms: u64,
    archive_reads: u64,
    archive_ms: u64,
    plugin_reads: u64,
    plugin_ms: u64,
    /// Slowest mods: (name, ms, loose files, archive entries).
    slowest: Vec<(String, u128, u32, u32)>,
    /// Mods re-analyzed from their cached index (only stale analyzers ran).
    indexed: usize,
    /// Analyzers of this run: id@version.
    analyzers: Vec<String>,
    /// Failures of optional/degraded analyzers (first 200) and their total.
    issues: Vec<String>,
    issue_count: usize,
}

#[derive(Serialize)]
struct SyncSummary {
    /// Project = canon (stored, imported from modlist.txt on first run), side = the mods folder (ADR 0006).
    /// Renames are found from the second run on (signatures recorded by the previous one).
    matched: usize,
    pending: usize,
    fs_created_mods: usize,
    fs_created_separators: usize,
    fs_deleted_mods: usize,
    fs_deleted_separators: usize,
    fs_renamed: usize,
    sample: Vec<String>,
}

#[derive(Serialize)]
struct Report {
    instance: InstanceInfo,
    sync: SyncSummary,
    manager_diff: ManagerDiff,
    mods_scanned: usize,
    from_cache: usize,
    missing: usize,
    files: u64,
    archive_entries: u64,
    paths: usize,
    providers: usize,
    conflict_paths: usize,
    chains: usize,
    contexts: usize,
    fully_overridden: usize,
    /// name -> [win, lose, mid, unique]
    mods: BTreeMap<String, [u32; 4]>,
    timings_ms: BTreeMap<&'static str, u128>,
    scan: ScanBreakdown,
    peak_rss_mib: Option<u64>,
    rss_after_mib: Option<u64>,
    /// Run lifecycle: final status, seal violations, transitions.
    run: RunState,
    seals: Vec<String>,
    /// Mods whose facts are degraded (unreadable subfolder, …).
    degraded: Vec<String>,
}

use sat_core::run::{RunState, RunStatus, RunSummary, StagedMod, StagedRoute, seal};
use sat_core::sync::names::is_separator_folder;

fn subfolders(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn analyze(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let all_folders = !has(args, "--listed-only");
    let manager = read_modlist(&inst.profile_dir().join("modlist.txt"))?;
    // The project is the canon (ADR 0006); modlist.txt is a side. First run imports it.
    let project_path = flag_value(args, "--project")
        .map(PathBuf::from)
        .unwrap_or_else(|| inst.profile_dir().join("satellite").join(PROJECT_FILE));
    let mut project = match std::fs::read(&project_path) {
        Ok(bytes) if !has(args, "--reimport") => {
            let p: Project =
                serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", project_path.display()))?;
            if p.schema != PROJECT_SCHEMA {
                return Err(format!(
                    "{}: project schema {} (this build reads {PROJECT_SCHEMA}); use --reimport",
                    project_path.display(),
                    p.schema
                ));
            }
            eprintln!("project: {}", project_path.display());
            p
        }
        Ok(bytes) => {
            // Re-import keeps what the project learned (signatures, decisions), replaces only the canon.
            let old: Option<Project> = serde_json::from_slice(&bytes).ok();
            let mut p = Project::import(manager.clone());
            if let Some(old) = old {
                p.signatures = old.signatures;
                p.sync = old.sync;
            }
            eprintln!(
                "project: re-imported canon from modlist.txt into {}",
                project_path.display()
            );
            p
        }
        Err(_) => {
            eprintln!(
                "project: imported from modlist.txt (first run) → {}",
                project_path.display()
            );
            Project::import(manager.clone())
        }
    };
    let manager_diff = project.manager_diff(&manager);
    if !manager_diff.is_empty() {
        eprintln!(
            "project vs modlist.txt: only in modlist={} only in project={} enabled differs={} order differs={} — the project stays canon; --reimport to take modlist.txt",
            manager_diff.only_manager.len(),
            manager_diff.only_project.len(),
            manager_diff.enabled_differs.len(),
            manager_diff.order_differs
        );
    }
    let list = project.canon.clone();

    // Inventory: what the profile says vs. what is on disk.
    let listed: std::collections::HashSet<String> =
        list.entries.iter().map(|e| e.name.to_lowercase()).collect();
    let mut roots = Vec::new();
    let mut unlisted: Vec<(String, PathBuf)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for root in [inst.mods_dir.as_path()] {
        let names = subfolders(root);
        roots.push(RootInfo {
            path: root.display().to_string(),
            exists: root.is_dir(),
            folders: names.len(),
            separator_folders: names.iter().filter(|n| is_separator_folder(n)).count(),
        });
        for n in names {
            let key = n.to_lowercase();
            if !is_separator_folder(&n) && !listed.contains(&key) && seen.insert(key) {
                unlisted.push((n.clone(), root.join(&n)));
            }
        }
    }
    let mods_in_list = list.entries.iter().filter(|e| e.is_mod()).count();
    let enabled = list.entries.iter().filter(|e| e.is_mod() && e.enabled).count();
    let info = InstanceInfo {
        profile: inst.profile.clone(),
        profiles: inst.profiles.clone(),
        roots,
        modlist_mods: mods_in_list,
        modlist_enabled: enabled,
        modlist_separators: list
            .entries
            .iter()
            .filter(|e| e.kind == EntryKind::Separator)
            .count(),
        modlist_foreign: list
            .entries
            .iter()
            .filter(|e| e.kind == EntryKind::Foreign)
            .count(),
        unlisted_folders: unlisted.len(),
        unlisted_sample: unlisted.iter().take(20).map(|(n, _)| n.clone()).collect(),
        unlisted_scanned: all_folders,
    };
    eprintln!(
        "profile={:?} (available: {})",
        info.profile,
        info.profiles.join(", ")
    );
    for r in &info.roots {
        eprintln!(
            "root {} exists={} folders={} (separators {})",
            r.path, r.exists, r.folders, r.separator_folders
        );
    }
    eprintln!(
        "modlist: mods={} enabled={} separators={} foreign={}; folders on disk not in modlist={}",
        info.modlist_mods,
        info.modlist_enabled,
        info.modlist_separators,
        info.modlist_foreign,
        info.unlisted_folders
    );
    if enabled == 0 {
        eprintln!(
            "WARNING: no mod is enabled in profile {:?} — conflicts will be empty. Pick the profile with --profile NAME.",
            info.profile
        );
    }

    let mut run = RunState::default();
    let mut jobs: Vec<ScanJob> = list
        .entries
        .iter()
        .filter(|e| e.is_mod())
        .map(|e| ScanJob {
            name: e.name.clone(),
            path: inst.mod_dir(&e.name),
        })
        .collect();
    if all_folders {
        jobs.extend(unlisted.into_iter().map(|(name, path)| ScanJob { name, path }));
    }
    jobs.push(ScanJob {
        name: OVERWRITE.into(),
        path: inst.overwrite_dir.clone(),
    });

    let opts = ScanOptions {
        cache_dir: Some(
            flag_value(args, "--cache")
                .map(PathBuf::from)
                .unwrap_or_else(|| inst.profile_dir().join("satellite").join("cache")),
        ),
        force: has(args, "--force"),
        record_scan: has(args, "--records"),
        io_threads: flag_value(args, "--io-threads")
            .map(|v| v.parse().map_err(|_| format!("--io-threads: not a number: {v}")))
            .transpose()?
            .unwrap_or(0),
        ..Default::default()
    };
    let mut timings = BTreeMap::new();
    let progress = ScanProgress::default();
    let t = Instant::now();
    let done = std::sync::atomic::AtomicBool::new(false);
    run.go(RunStatus::Running, "scan").map_err(|e| e.to_string())?;
    let (mut issues, mut issue_count) = (Vec::<String>::new(), 0usize);
    let outcomes = std::thread::scope(|s| {
        s.spawn(|| {
            let mut last = Instant::now();
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if last.elapsed().as_secs() >= 5 {
                    last = Instant::now();
                    let (d, total, cached, files) = progress.snapshot();
                    eprintln!(
                        "  scan {d}/{total} mods, {cached} from cache, {files} files, {:.0} s",
                        t.elapsed().as_secs_f64()
                    );
                }
            }
        });
        // Facts are folded into the table batch by batch and dropped: peak memory = table + one batch.
        let mut builder = TableBuilder::new();
        let mut seen = Vec::with_capacity(jobs.len());
        sat_scan::scan_batched(&jobs, &opts, &progress, SCAN_BATCH, |_, o| {
            builder.add(&o.facts);
            issue_count += o.issues.len();
            for i in &o.issues {
                if issues.len() < 200 {
                    issues.push(format!("{}: {i}", o.facts.name));
                }
            }
            seen.push(Seen {
                name: o.facts.name.clone(),
                missing: o.facts.missing,
                from_cache: o.from_cache,
                route: match o.route {
                    sat_scan::Route::Scan => StagedRoute::Scan,
                    sat_scan::Route::Indexed { .. } => StagedRoute::Indexed,
                    sat_scan::Route::Reuse => StagedRoute::Reuse,
                },
                error: o.facts.error.clone(),
                signature: o.signature,
                files: o.facts.metrics.files,
                archive_files: o.facts.metrics.archive_files,
                elapsed: o.elapsed,
            });
        });
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        (seen, builder)
    });
    let (outcomes, builder) = outcomes;
    timings.insert("scan", t.elapsed().as_millis());
    eprintln!("after scan: rss={:?}MiB", rss_mib());

    let ms = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000;
    let n = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
    let mut slowest: Vec<(String, u128, u32, u32)> = outcomes
        .iter()
        .filter(|o| !o.from_cache)
        .map(|o| (o.name.clone(), o.elapsed.as_millis(), o.files, o.archive_files))
        .collect();
    slowest.sort_by_key(|b| std::cmp::Reverse(b.1));
    slowest.truncate(20);
    let breakdown = ScanBreakdown {
        threads: if opts.io_threads == 0 {
            sat_scan::default_io_threads()
        } else {
            opts.io_threads
        },
        signature_ms: ms(&progress.signature_ns),
        cache_load_ms: ms(&progress.cache_load_ns),
        walk_ms: ms(&progress.walk_ns),
        dds_reads: n(&progress.dds_reads),
        dds_ms: ms(&progress.dds_ns),
        archive_reads: n(&progress.archive_reads),
        archive_ms: ms(&progress.archive_ns),
        plugin_reads: n(&progress.plugin_reads),
        plugin_ms: ms(&progress.plugin_ns),
        slowest,
        indexed: progress.indexed.load(std::sync::atomic::Ordering::Relaxed),
        analyzers: sat_scan::Registry::builtin(&opts)
            .stamps()
            .iter()
            .map(|s| format!("{}@{}", s.id, s.version))
            .collect(),
        issues,
        issue_count,
    };
    eprintln!(
        "analyzers: {}; indexed={} analyzer issues={}",
        breakdown.analyzers.join(" "),
        breakdown.indexed,
        breakdown.issue_count
    );
    eprintln!(
        "scan thread-time ms (threads={}): signature={} cache_load={} walk={} [dds {}x {} | archives {}x {} | plugins {}x {}]",
        breakdown.threads,
        breakdown.signature_ms,
        breakdown.cache_load_ms,
        breakdown.walk_ms,
        breakdown.dds_reads,
        breakdown.dds_ms,
        breakdown.archive_reads,
        breakdown.archive_ms,
        breakdown.plugin_reads,
        breakdown.plugin_ms
    );

    let sync = {
        use sat_core::sync::records::{DiskFolder, EntityKind, Mismatch};
        let sig: std::collections::HashMap<String, u64> = outcomes
            .iter()
            .filter(|o| !o.missing && o.name != OVERWRITE)
            .map(|o| (o.name.to_lowercase(), o.signature))
            .collect();
        let disk: Vec<DiskFolder> = subfolders(&inst.mods_dir)
            .into_iter()
            .map(|name| DiskFolder {
                signature: sig.get(&name.to_lowercase()).copied(),
                name,
            })
            .collect();
        let r = project.observe(&disk);
        let count = |pred: &dyn Fn(&Mismatch, EntityKind) -> bool| {
            r.records.iter().filter(|x| pred(&x.mismatch, x.entity)).count()
        };
        let summary = SyncSummary {
            matched: r.matched.len(),
            pending: r.count(sat_core::sync::records::Status::Pending),
            fs_created_mods: count(&|m, k| matches!(m, Mismatch::FsCreated { .. }) && k == EntityKind::Mod),
            fs_created_separators: count(&|m, k| {
                matches!(m, Mismatch::FsCreated { .. }) && k == EntityKind::Separator
            }),
            fs_deleted_mods: count(&|m, k| matches!(m, Mismatch::FsDeleted { .. }) && k == EntityKind::Mod),
            fs_deleted_separators: count(&|m, k| {
                matches!(m, Mismatch::FsDeleted { .. }) && k == EntityKind::Separator
            }),
            fs_renamed: count(&|m, _| matches!(m, Mismatch::FsRenamed { .. })),
            sample: r.records.iter().take(30).map(|x| x.fingerprint.clone()).collect(),
        };
        eprintln!(
            "sync: matched={} created mods/separators={}/{} deleted mods/separators={}/{} renamed={}",
            summary.matched,
            summary.fs_created_mods,
            summary.fs_created_separators,
            summary.fs_deleted_mods,
            summary.fs_deleted_separators,
            summary.fs_renamed
        );
        summary
    };

    let from_cache = outcomes.iter().filter(|o| o.from_cache).count();
    let missing = outcomes
        .iter()
        .filter(|o| o.missing && o.name != OVERWRITE)
        .count();
    let files: u64 = outcomes.iter().map(|o| u64::from(o.files)).sum();
    let archive_entries: u64 = outcomes.iter().map(|o| u64::from(o.archive_files)).sum();

    let t = Instant::now();
    let table = builder.finish();
    timings.insert("table_build", t.elapsed().as_millis());
    eprintln!("live table heap={}MiB", table.heap_bytes() >> 20);

    let index: BTreeMap<String, ModId> = table
        .mod_names()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
        .collect();
    let prio = priorities(&list, &table, &index);

    let t = Instant::now();
    let proj = OverwriteProjection::compute(&table, prio.clone());
    timings.insert("overwrite_compute", t.elapsed().as_millis());
    let t = Instant::now();
    let _again = OverwriteProjection::compute(&table, prio);
    timings.insert("overwrite_recompute", t.elapsed().as_millis());

    let totals = proj.totals();
    let ts = table.stats();
    let mods = table
        .mod_names()
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let s = proj.stat(ModId::from_idx(i));
            (n.clone(), [s.win, s.lose, s.mid, s.unique])
        })
        .collect();
    // Seal before anything is published (project file); a failed seal publishes nothing.
    run.go(RunStatus::Staging, "projection built")
        .map_err(|e| e.to_string())?;
    let summary = RunSummary {
        expected: jobs.iter().map(|j| j.name.clone()).collect(),
        staged: outcomes
            .iter()
            .map(|o| StagedMod {
                name: o.name.clone(),
                route: o.route,
                missing: o.missing,
                error: o.error.clone(),
            })
            .collect(),
        chains: totals.chains,
        contexts: totals.contexts,
        conflict_paths: totals.conflict_paths,
    };
    let seals = seal(&summary);
    let degraded: Vec<String> = outcomes
        .iter()
        .filter_map(|o| o.error.as_ref().map(|e| format!("{}: {e}", o.name)))
        .collect();
    let publish = seals.is_empty() && !has(args, "--no-save");
    if seals.is_empty() {
        for (to, why) in [
            (RunStatus::Committing, "seals passed"),
            (RunStatus::Verifying, "project written"),
            (RunStatus::Promoting, "verified"),
            (RunStatus::Completed, "published"),
        ] {
            run.go(to, why).map_err(|e| e.to_string())?;
        }
    } else {
        run.go(RunStatus::Failed, "seals failed")
            .map_err(|e| e.to_string())?;
        eprintln!("SEALS FAILED — nothing published:");
        for v in &seals {
            eprintln!("  {v}");
        }
    }
    if !degraded.is_empty() {
        eprintln!("degraded mods: {} (first: {})", degraded.len(), degraded[0]);
    }
    let report = Report {
        instance: info,
        sync,
        manager_diff: manager_diff.clone(),
        mods_scanned: jobs.len(),
        from_cache,
        missing,
        files,
        archive_entries,
        paths: ts.paths,
        providers: ts.providers,
        conflict_paths: totals.conflict_paths,
        chains: totals.chains,
        contexts: totals.contexts,
        fully_overridden: totals.fully_overridden,
        mods,
        timings_ms: timings,
        scan: breakdown,
        peak_rss_mib: peak_rss_mib(),
        rss_after_mib: rss_mib(),
        run,
        seals,
        degraded,
    };
    eprintln!(
        "mods={} cache={} missing={} files={} archive={} paths={} providers={} conflicts={} chains={} contexts={} X={}",
        report.mods_scanned,
        report.from_cache,
        report.missing,
        report.files,
        report.archive_entries,
        report.paths,
        report.providers,
        report.conflict_paths,
        report.chains,
        report.contexts,
        report.fully_overridden
    );
    eprintln!(
        "timings_ms={:?} peak_rss_mib={:?} rss_after_mib={:?}",
        report.timings_ms, report.peak_rss_mib, report.rss_after_mib
    );
    if let Some(out) = flag_value(args, "--report") {
        std::fs::write(
            out,
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    if publish {
        let bytes = serde_json::to_vec_pretty(&project).map_err(|e| e.to_string())?;
        sat_mo2::write_atomic(&project_path, &bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `<instance>` is an MO2 folder (with or without `ModOrganizer.ini`). `--mods DIR --profiles DIR
/// [--overwrite DIR]` give the folders directly, like the monolith's folder pickers; then `<instance>` is optional.
fn open_instance(args: &[String]) -> Result<Instance, String> {
    let explicit = |flag: &str| flag_value(args, flag).map(PathBuf::from);
    let path = args.first().filter(|a| !a.starts_with("--"));
    let mut inst = match (explicit("--mods"), explicit("--profiles"), path) {
        (Some(m), Some(p), _) => {
            let ow = explicit("--overwrite").unwrap_or_else(|| {
                m.parent()
                    .map_or_else(|| PathBuf::from("overwrite"), |b| b.join("overwrite"))
            });
            Instance::from_dirs(m, p, ow, None)
        }
        (_, _, Some(path)) => {
            let mut i = Instance::load(path).map_err(|e| e.to_string())?;
            if let Some(m) = explicit("--mods") {
                i.mods_dir = m;
            }
            if let Some(o) = explicit("--overwrite") {
                i.overwrite_dir = o;
            }
            i
        }
        _ => return Err("missing <instance> (or --mods DIR --profiles DIR)".into()),
    };
    if let Some(p) = flag_value(args, "--profile") {
        if !inst.profiles.iter().any(|x| x == p) {
            return Err(format!(
                "profile {p:?} not found; available: {}",
                inst.profiles.join(", ")
            ));
        }
        inst.profile = p.to_string();
    }
    Ok(inst)
}

fn read_modlist(path: &std::path::Path) -> Result<Modlist, String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(profile::parse_modlist(&data).0)
}

#[derive(Serialize)]
struct DonorReport {
    target_profile: String,
    target_mods: usize,
    donor_mods_dir: String,
    donor_modlist: Option<String>,
    validation: Option<donor::Validation>,
    counts: BTreeMap<String, usize>,
    candidates: Vec<donor::Candidate>,
}

/// External donor (monolith `ExternalDonorSession`): matches another mods folder (and optionally its
/// `modlist.txt`) against the instance's profile, which stands in for the project until NG-D002.
fn donor(args: &[String]) -> Result<(), String> {
    let inst = open_instance(args)?;
    let mods_dir = PathBuf::from(flag_value(args, "--donor-mods").ok_or("missing --donor-mods DIR")?);
    if !mods_dir.is_dir() {
        return Err(format!("not a folder: {}", mods_dir.display()));
    }
    let target_list = read_modlist(&inst.profile_dir().join("modlist.txt"))?;
    let mut sep = String::new();
    let mut targets = Vec::new();
    for e in &target_list.entries {
        match e.kind {
            EntryKind::Separator => sep = e.name.trim_end_matches(profile::SEPARATOR_SUFFIX).to_string(),
            EntryKind::Mod => targets.push(donor::TargetMod {
                name: e.name.clone(),
                separator: sep.clone(),
            }),
            EntryKind::Foreign => {}
        }
    }
    let target = donor::TargetIndex::new(targets);

    let t = Instant::now();
    let folders = subfolders(&mods_dir);
    let donor_list = match flag_value(args, "--donor-modlist") {
        Some(p) => read_modlist(std::path::Path::new(p))?,
        None => Modlist::default(),
    };
    let validation = flag_value(args, "--donor-modlist").map(|_| donor::validate(&donor_list, &folders));
    let candidates = donor::candidates(&donor_list, &folders, &target);
    let elapsed = t.elapsed();

    let mut counts = BTreeMap::new();
    for c in &candidates {
        *counts.entry(format!("{:?}", c.kind).to_lowercase()).or_insert(0) += 1;
    }
    eprintln!(
        "target profile={:?} mods={}; donor folder {} : folders={} (separators {})",
        inst.profile,
        target.targets.len(),
        mods_dir.display(),
        folders.len(),
        folders.iter().filter(|f| is_separator_folder(f)).count()
    );
    if let Some(v) = &validation {
        eprintln!(
            "donor modlist fit: {}% ({}) matched={}/{} folders={} modlist_only={} folder_only={}",
            v.score,
            v.tone(),
            v.matched,
            v.modlist_total,
            v.folder_total,
            v.modlist_only.len(),
            v.folder_only.len()
        );
    }
    eprintln!("candidates: {counts:?} in {} ms", elapsed.as_millis());
    if let Some(out) = flag_value(args, "--report") {
        let report = DonorReport {
            target_profile: inst.profile.clone(),
            target_mods: target.targets.len(),
            donor_mods_dir: mods_dir.display().to_string(),
            donor_modlist: flag_value(args, "--donor-modlist").map(str::to_string),
            validation,
            counts,
            candidates,
        };
        std::fs::write(
            out,
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Prints what the format parsers see in real files (archives, plugins, textures). For checking parsers on samples.
fn inspect(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("usage: sat-cli inspect <file>...".into());
    }
    for path in args {
        let p = std::path::Path::new(path);
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let open = || std::fs::File::open(p).map_err(|e| format!("{path}: {e}"));
        println!("== {path}");
        match ext.as_str() {
            "bsa" | "ba2" => {
                let m = sat_formats::read_archive(open()?).map_err(|e| format!("{path}: {e}"))?;
                let size: u64 = m.entries.iter().map(|e| e.size).sum();
                let vram: u64 = m.entries.iter().map(|e| e.vram).sum();
                println!(
                    "kind={:?} entries={} sum_size={size} sum_vram={vram}",
                    m.kind,
                    m.entries.len()
                );
                for e in m.entries.iter().take(5) {
                    println!("  {} size={} vram={}", e.path, e.size, e.vram);
                }
            }
            "esp" | "esm" | "esl" => {
                let h = sat_formats::read_plugin_header(std::io::BufReader::new(open()?))
                    .map_err(|e| format!("{path}: {e}"))?;
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                println!("{h:?}\nkind={:?}", h.kind(&name));
                let r = sat_formats::scan_records(std::io::BufReader::with_capacity(1 << 20, open()?))
                    .map_err(|e| format!("{path}: {e}"))?;
                let mut top: Vec<_> = r.by_signature.iter().collect();
                top.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
                println!(
                    "records total={} placed_refs={} top={:?}",
                    r.total,
                    r.placed_references(),
                    &top[..top.len().min(8)]
                );
            }
            "dds" => {
                let data = std::fs::read(p).map_err(|e| format!("{path}: {e}"))?;
                match sat_formats::DdsInfo::parse(&data) {
                    Some(i) => println!("{i:?} known={}", i.known()),
                    None => println!("not a DDS header ({} bytes)", data.len()),
                }
            }
            _ => println!("unknown extension"),
        }
    }
    Ok(())
}

fn priorities(list: &Modlist, table: &ProviderTable, index: &BTreeMap<String, ModId>) -> Priorities {
    list.priorities(
        table.mod_count(),
        |n| index.get(&n.to_lowercase()).copied(),
        index.get(&OVERWRITE.to_lowercase()).copied(),
    )
}

/// Deterministic xorshift for synthetic data.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }
}

const AREAS: &[&str] = &[
    "textures/actors/character/female",
    "textures/actors/character/male",
    "textures/armor/steel",
    "textures/armor/daedric",
    "textures/landscape",
    "textures/landscape/trees",
    "textures/architecture/whiterun",
    "textures/architecture/windhelm",
    "textures/clutter",
    "textures/effects",
    "textures/terrain/lods",
    "meshes/actors/character/character assets",
    "meshes/armor/steel",
    "meshes/landscape/trees",
    "meshes/architecture/whiterun",
    "meshes/clutter",
    "meshes/actors/character/animations",
    "scripts",
    "interface",
    "sound/fx",
    "seq",
    "skse/plugins",
    "textures/sky",
    "meshes/weapons",
    "textures/weapons",
    "lodsettings",
    "grass",
    "meshes/terrain/tamriel",
    "textures/terrain/tamriel",
];
const EXTS: &[&str] = &[
    ".dds", ".dds", ".dds", ".dds", ".nif", ".nif", ".pex", ".hkx", ".wav", ".xml", ".json", ".ini",
];

/// Same distribution as the Go spike's `synth.Mods` (skewed mod sizes, 30% shared vocabulary).
fn synth(mods: usize, files_per_mod: usize, seed: u64) -> Vec<ModFacts> {
    let mut r = Rng(seed | 1);
    (0..mods)
        .map(|i| {
            let x = r.below(1000) as f64 / 1000.0;
            let cnt = if x < 0.02 {
                files_per_mod * 40
            } else if x < 0.15 {
                files_per_mod * 4
            } else if x < 0.5 {
                files_per_mod
            } else {
                files_per_mod / 4
            };
            let home = r.below(AREAS.len() as u64) as usize;
            let mut dirs: BTreeMap<String, BTreeMap<String, LooseFile>> = BTreeMap::new();
            for f in 0..cnt {
                let area = if r.chance(0.3) {
                    AREAS[r.below(AREAS.len() as u64) as usize]
                } else {
                    AREAS[home]
                };
                let ext = EXTS[r.below(EXTS.len() as u64) as usize];
                let name = if r.chance(0.3) {
                    format!("shared_{:04}{ext}", r.below(300))
                } else {
                    format!("m{i:05}_{f:06}{ext}")
                };
                let dir = format!("{area}/set{:02}", r.below(4));
                let size = r.below(4 << 20);
                let vram = r.below(5000) * 1024;
                dirs.entry(dir)
                    .or_default()
                    .entry(name.to_lowercase())
                    .or_insert(LooseFile { name, size, vram });
            }
            ModFacts {
                name: format!("Mod {i:05}"),
                dirs: dirs
                    .into_iter()
                    .map(|(rel, fs)| LooseDir {
                        rel,
                        files: fs.into_values().collect(),
                    })
                    .collect(),
                ..Default::default()
            }
        })
        .collect()
}

fn bench_synth(args: &[String]) -> Result<(), String> {
    let mods: usize = flag_value(args, "--mods")
        .map_or(Ok(5000), str::parse)
        .map_err(|e| format!("--mods: {e}"))?;
    let files: usize = flag_value(args, "--files")
        .map_or(Ok(1100), str::parse)
        .map_err(|e| format!("--files: {e}"))?;
    let rounds: usize = flag_value(args, "--rounds")
        .map_or(Ok(3), str::parse)
        .map_err(|e| format!("--rounds: {e}"))?;
    let t = Instant::now();
    let facts = synth(mods, files, 42);
    let providers: usize = facts.iter().map(ModFacts::provider_count).sum();
    eprintln!(
        "synth: mods={mods} providers={providers} in {:?} rss={:?}MiB",
        t.elapsed(),
        rss_mib()
    );
    let t = Instant::now();
    let table = ProviderTable::build(&facts);
    let s = table.stats();
    eprintln!(
        "build: paths={} providers={} dirs={} names={} in {:?}",
        s.paths,
        s.providers,
        s.dirs,
        s.names,
        t.elapsed()
    );
    drop(facts);
    eprintln!(
        "table only: live table heap={}MiB rss={:?}MiB peak={:?}MiB",
        table.heap_bytes() >> 20,
        rss_mib(),
        peak_rss_mib()
    );
    let mut prio = Priorities((0..mods as i32).collect());
    for round in 0..rounds {
        let t = Instant::now();
        let p = OverwriteProjection::compute(&table, prio.clone());
        let tt = p.totals();
        eprintln!(
            "compute#{round}: conflict_paths={} chains={} contexts={} X={} in {:?}",
            tt.conflict_paths,
            tt.chains,
            tt.contexts,
            tt.fully_overridden,
            t.elapsed()
        );
        // reorder deterministically, like the Go benchmark
        prio = Priorities((0..mods).map(|i| ((i * 7919 + round) % mods) as i32).collect());
    }
    eprintln!(
        "after compute: rss={:?}MiB peak={:?}MiB threads={}",
        rss_mib(),
        peak_rss_mib(),
        rayon::current_num_threads()
    );
    Ok(())
}
