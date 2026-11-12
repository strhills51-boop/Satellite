//! `sat-cli` — developer tool for NG-D001 evidence. @sat:cli
//!
//! ```text
//! sat-cli analyze <instance> | --mods DIR --profiles DIR [--overwrite DIR]  [--profile NAME] [--listed-only] [--find EXPR [--limit N]] [--content] [--identical] [--no-hashes] [--min-size SIZE] [--records [--limit N]] [--project FILE] [--reimport] [--no-save] [--io-threads N] [--cache DIR] [--force] [--records] [--report FILE]
//! sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--as NAME] [--list] [--report FILE] [--take NAME]... [--take-ready] [--separator SEP | --after MOD] [--move]
//! sat-cli inspect <file.bsa|.ba2|.esp|.esm|.esl|.dds>...
//! sat-cli bench-synth [--mods N] [--files F] [--rounds R]
//! ```
//!
//! Owns: the developer CLI: analyze, donor, inspect, bench-synth.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use sat_core::canon::{EntryKind, Modlist};
use sat_core::facts::{LooseDir, LooseFile, ModFacts};
use sat_core::ids::ModId;
use sat_core::overwrite::{ConflictIndex, OverwriteProjection, Priorities};
use sat_core::plugins::{PluginCanon, PluginList, PluginVersions};
use sat_core::project::{ManagerDiff, Project};
use sat_core::table::{ProviderTable, TableBuilder};
use sat_mo2::{Instance, profile};
use sat_scan::{ScanJob, ScanOptions, ScanProgress};
use serde::Serialize;

use sat_core::session::OVERWRITE;
/// Provisional project file until the container format is decided (ADR 0007).
const PROJECT_FILE: &str = sat_stll::FILE_NAME;
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
    /// Scan aggregates, kept only for `--find` (filters need them after the facts are dropped).
    metrics: Option<sat_core::facts::ModMetrics>,
    /// Tags found by the rule set (`--tags`), best first.
    tags: Vec<String>,
}

/// Millions of small allocations (path strings, per-dir vectors) dominate table builds; the system allocator on
/// Windows is markedly slower at that than mimalloc.
mod bench_order;
mod commit_cmd;
mod donor_cmd;
mod factory_cmd;
mod ignore_cmd;
mod intent_cmd;
mod patch_cmd;
mod plugins_cmd;
mod project_io;
mod stickers_cmd;
mod verify_cmd;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    // invisible characters a copy from a phone or a chat brings along (no-break and zero-width spaces, BOM) are no
    // part of a command
    let args: Vec<String> = std::env::args()
        .skip(1)
        .map(|a| {
            a.chars()
                .filter(|c| !matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}' | '\u{FEFF}'))
                .map(|c| if c == '\u{00A0}' { ' ' } else { c })
                .collect::<String>()
                .trim()
                .to_string()
        })
        .filter(|a| !a.is_empty())
        .collect();
    let res = match args.first().map(String::as_str) {
        Some("analyze") => analyze(&args[1..]),
        Some("donor") => donor_cmd::donor(&args[1..]),
        Some("sync") => commit_cmd::sync(&args[1..]),
        Some("commit") => commit_cmd::commit(&args[1..]),
        Some("ignore") => ignore_cmd::ignore(&args[1..]),
        Some("plan-install") => intent_cmd::plan_install(&args[1..]),
        Some("stickers") => stickers_cmd::stickers(&args[1..]),
        Some("plugins") => plugins_cmd::plugins(&args[1..]),
        Some("verify-formats") => verify_cmd::verify(&args[1..]),
        Some("group") => factory_cmd::group(&args[1..]),
        Some("factory") => factory_cmd::factory(&args[1..]),
        Some("patch") => patch_cmd::patch(&args[1..]),
        Some("counters") => stickers_cmd::counters(&args[1..]),
        Some("inspect") => inspect(&args[1..]),
        Some("bench-synth") => bench_synth(&args[1..]),
        Some("bench-order") => bench_order::run(&args[1..]),
        other => Err(format!(
            "{}usage: sat-cli analyze <instance> | --mods DIR --profiles DIR [--overwrite DIR]  [--profile NAME] [--listed-only] [--find EXPR [--limit N]] [--content] [--identical] [--no-hashes] [--min-size SIZE] [--records [--limit N]] [--project FILE] [--reimport] [--no-save] [--io-threads N] [--cache DIR] [--force] [--report FILE]\n       sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--as NAME] [--list] [--report FILE] [--take NAME]... [--take-ready] [--separator SEP | --after MOD] [--move]\n       sat-cli sync <instance> [--profile NAME] [--decide N ANSWER]...\n       sat-cli commit <instance> [--profile NAME] [--apply] [--confirm OUTPUT]...\n       sat-cli ignore <instance> [--add RULE]... [--remove RULE]...\n       sat-cli plugins <instance> [--move NAME]... [--before NAME | --last] [--check NAME]... [--uncheck NAME]... [--hide NAME]... [--show NAME]... [--cascade] [--list]\n       sat-cli stickers <instance> [--add \"STICKER=EXPR\"]... [--remove STICKER]...\n       sat-cli counters <instance> [--add \"ID=TEMPLATE\"]... [--case \"ID|CONDITION|TONE\"]... [--show-if \"ID|CONDITION\"]... [--remove ID]...\n       sat-cli group <instance> [--add NAME --from MOD --to MOD] [--remove NAME]\n       sat-cli factory <instance> [--add \"GROUP=OUTPUT\" [--link] [--keep-members] [--seal] [--pack] [--downscale N]] [--remove OUTPUT]\n       sat-cli patch <instance> [--mod MOD --file PATH [--set \"SECTION/KEY=VALUE\"]... [--unset \"SECTION/KEY\"]... [--rebase] [--drop]]\n       sat-cli plan-install <instance> --archive FILE [--name NAME] [--after MOD] [--disabled] [--choose STEP/GROUP/OPTION]... [--show]\n       sat-cli inspect <file>...\n       sat-cli verify-formats <dir> [--dds N] [--all] [--out FILE]\n       sat-cli bench-synth [--mods N] [--files F] [--rounds R]\n       sat-cli bench-order [--entries N] [--block B]",
            other.map_or_else(String::new, |o| format!("unknown command {o:?}\n"))
        )),
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
    /// Normalized chains (route groups; the monolith's chain count).
    normalized: usize,
    contexts: usize,
    fully_overridden: usize,
    plugins: sat_core::plugins::PluginStats,
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
    // a filter is checked before the scan: a typo should not cost a scan of the build
    let find = flag_value(args, "--find")
        .map(|src| {
            sat_core::expr::Expr::compile_with(src, &sat_core::metrics::resolve).map_err(|e| {
                format!(
                    "--find: {e}\n  {src}\n  {}^\n  metrics: {}, ext.<extension>, dir.<top folder>",
                    " ".repeat(src[..e.at].chars().count()),
                    sat_core::metrics::MOD_METRICS
                        .iter()
                        .map(|d| d.id)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .transpose()?;
    let keep_metrics = find.is_some();
    // @sat:cli.tags
    // tags from a rule set: the built-in one (the monolith's detectors), or a file given with --tag-rules
    let rules = if has(args, "--tags") || flag_value(args, "--tag-rules").is_some() {
        Some(match flag_value(args, "--tag-rules") {
            Some(f) => {
                let text = std::fs::read_to_string(f).map_err(|e| format!("--tag-rules {f}: {e}"))?;
                let set = serde_json::from_str(&text).map_err(|e| format!("--tag-rules {f}: {e}"))?;
                sat_core::tag_rules::Compiled::new(set)?
            }
            None => sat_core::tag_rules::Compiled::builtin(),
        })
    } else {
        None
    };
    let manager = read_modlist(&inst.profile_dir().join("modlist.txt"))?;
    // The project is the canon (ADR 0006); modlist.txt is a side. First run imports it.
    let project_path = flag_value(args, "--project")
        .map(PathBuf::from)
        .unwrap_or_else(|| inst.profile_dir().join("satellite").join(PROJECT_FILE));
    let (mut stll, stored) = match project_io::open(&project_path)? {
        Some((f, p)) => (f, Some(p)),
        None => (sat_stll::Stll::new(&project_path), None),
    };
    let mut project = match stored {
        Some(p) if !has(args, "--reimport") => {
            eprintln!("project: {}", project_path.display());
            p
        }
        Some(old) => {
            // Re-import keeps what the project learned and what the user decided (signatures, sync decisions,
            // decoration, settled chains), replaces only the canon. Decoration of names no longer in the canon is
            // reported when the book applies it.
            let mut p = Project::import(manager.clone());
            // entities keep their ids by name, so everything keyed by id still finds them
            p.entities = old.entities;
            p.entities.follow(&p.canon);
            p.signatures = old.signatures;
            p.sync = old.sync;
            p.decor = old.decor;
            p.decisions = old.decisions;
            p.plugins = old.plugins;
            p.conflict_ignore = old.conflict_ignore;
            p.tags = old.tags;
            p.intents = old.intents;
            p.sticker_rules = old.sticker_rules;
            p.counters = old.counters;
            p.sources = old.sources;
            p.new_mods = old.new_mods;
            p.extra = old.extra;
            eprintln!(
                "project: re-imported canon from modlist.txt into {}",
                project_path.display()
            );
            p
        }
        None => {
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

    // facts of the project (ADR 0007 step 2): written after a scan, read instead of one with --offline
    let offline = has(args, "--offline");
    let save_facts = !has(args, "--no-save") && !offline;
    let fact_ids: std::collections::HashMap<String, sat_core::entity::EntityId> = project
        .entities
        .iter()
        .map(|e| (e.name.to_lowercase(), e.id.clone()))
        .chain(std::iter::once((
            OVERWRITE.to_lowercase(),
            sat_core::entity::EntityId("overwrite".into()),
        )))
        .collect();
    let fact_id = |name: &str| fact_ids.get(&name.to_lowercase()).cloned();
    if offline {
        eprintln!(
            "offline: facts of {} mods from {} instead of a scan",
            stll.facts_count(),
            project_path.display()
        );
    }
    let planned: std::collections::HashSet<String> = project
        .planned()
        .into_iter()
        .map(|p| p.name.to_lowercase())
        .collect();
    // the project file being written while the scan runs: facts go in as they come, unchanged ones are copied
    let mut writer = if save_facts {
        Some(stll.begin().map_err(|e| e.to_string())?)
    } else {
        None
    };
    // offline, the facts are read from the same file through a handle of its own
    let mut source = if offline && project_path.exists() {
        Some(sat_stll::Stll::open(&project_path).map_err(|e| e.to_string())?.0)
    } else {
        None
    };
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
        let mut plugin_versions = PluginVersions::default();
        let mut seen = Vec::with_capacity(jobs.len());
        let mut sink = |mut o: sat_scan::ScanOutcome| {
            // a planned mod has no folder yet: its facts come from its archive (`sat-cli plan-install`)
            if o.facts.missing
                && planned.contains(&o.facts.name.to_lowercase())
                && let Some(eid) = fact_id(&o.facts.name)
                && let Some((sig, f)) = writer.as_mut().and_then(|w| w.stored_facts(&eid.0))
            {
                o.facts = f;
                o.signature = sig;
            }
            // @sat:cli.facts
            // the project keeps what the scan saw, by entity id, so it works without the build
            if !o.facts.missing
                && let Some(eid) = fact_id(&o.facts.name)
                && let Some(w) = writer.as_mut()
                && let Err(e) = w.put_facts(&eid.0, o.signature, &o.facts)
                && issues.len() < 200
            {
                issues.push(format!("{}: project facts not saved: {e}", o.facts.name));
            }
            let id = builder.add(&o.facts);
            plugin_versions.add(id, &o.facts);
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
                metrics: (keep_metrics && !o.facts.missing).then(|| o.facts.metrics.clone()),
                tags: rules.as_ref().map_or_else(Vec::new, |r| {
                    r.tags(&o.facts).into_iter().map(|(t, _)| t).collect()
                }),
            });
        };
        if offline {
            // the build is not here: the project's facts stand in for the scan
            for job in &jobs {
                let stored = fact_id(&job.name).and_then(|eid| source.as_mut()?.facts(&eid.0));
                let (signature, facts) = stored.unwrap_or_else(|| (0, ModFacts::missing(job.name.clone())));
                progress.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                sink(sat_scan::ScanOutcome {
                    facts,
                    from_cache: true,
                    route: sat_scan::Route::Reuse,
                    issues: Vec::new(),
                    signature,
                    elapsed: std::time::Duration::ZERO,
                });
            }
        } else {
            sat_scan::scan_batched(&jobs, &opts, &progress, SCAN_BATCH, |_, o| sink(o));
        }
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        (seen, builder, plugin_versions)
    });
    let (outcomes, builder, plugin_versions) = outcomes;
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
        // offline there is no mods folder to compare with: the records of the last real run stand
        let r = if offline {
            sat_core::sync::records::Reconcile {
                records: project.sync.clone(),
                matched: Vec::new(),
            }
        } else {
            project.observe(&disk)
        };
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
    let conflict_index = std::sync::Arc::new(ConflictIndex::build_with(&table, &project.conflict_ignore));
    if conflict_index.ignored_count() > 0 {
        eprintln!(
            "conflict ignore: {} rule(s), {} contested path(s) left out",
            project.conflict_ignore.rules().len(),
            conflict_index.ignored_count()
        );
    }
    // logical groups of the project are computed as one mod (`@sat:overwrite.units`), as in the window
    let units = {
        let (book, _) = project.book();
        let gs: Vec<Vec<ModId>> = book
            .group_members()
            .into_iter()
            .map(|(_, _, g)| {
                g.into_iter()
                    .filter_map(|k| index.get(&book.name(k).to_lowercase()).copied())
                    .collect()
            })
            .collect();
        sat_core::overwrite::Units::of_groups(table.mod_count(), &gs)
    };
    let proj = OverwriteProjection::compute_units(conflict_index, &table, prio.clone(), units);
    timings.insert("overwrite_compute", t.elapsed().as_millis());
    let t = Instant::now();
    let _again = OverwriteProjection::compute(&table, prio.clone());
    timings.insert("overwrite_recompute", t.elapsed().as_millis());

    // @sat:cli.sources
    // where each mod comes from, as MO2 recorded it (`meta.ini`): read anew each run, offline kept from the project
    if !offline {
        use rayon::prelude::*;
        let t = Instant::now();
        let read: Vec<(String, sat_core::source::ModSource)> = project
            .canon
            .entries
            .par_iter()
            .filter(|e| e.is_mod())
            .filter_map(|e| {
                let m = sat_mo2::ModMeta::read(&inst.mod_dir(&e.name));
                let src = sat_core::source::ModSource {
                    site: "nexus".into(),
                    game: m.game_name,
                    mod_id: m.mod_id,
                    version: m.version,
                    newest_version: m.newest_version,
                    file: m.installation_file,
                    url: m.url,
                };
                (!src.is_empty()).then(|| (e.name.clone(), src))
            })
            .collect();
        project.sources = read
            .into_iter()
            .filter_map(|(n, s)| Some((project.entities.id_of(&n)?.0.clone(), s)))
            .collect();
        let updates = project.sources.values().filter(|s| s.update_available()).count();
        eprintln!(
            "sources: {} mods with a source (Nexus id {}), {updates} with a known update, {} ms",
            project.sources.len(),
            project.sources.values().filter(|s| s.mod_id.is_some()).count(),
            t.elapsed().as_millis()
        );
    }
    let sources: std::collections::HashMap<String, sat_core::source::ModSource> = project
        .sources_by_name()
        .into_iter()
        .map(|(n, s)| (n.to_lowercase(), s))
        .collect();

    // @sat:cli.identical
    // content of contested copies (`@sat:identical`): read where sizes match, hashes cached in the profile; every
    // conflict stays counted — this only says what the losing copies are
    let content: Option<Vec<sat_core::identical::ModContent>> = if offline || has(args, "--no-hashes") {
        None
    } else {
        let min = flag_value(args, "--min-size").map_or(Ok(0), parse_size)?;
        let t = Instant::now();
        let cache_file = inst.profile_dir().join("satellite").join("content-hashes.txt");
        let (hashes, read) = sat_scan::content::hashes_for(
            &table,
            |m| {
                if m == OVERWRITE {
                    inst.overwrite_dir.clone()
                } else {
                    inst.mods_dir.join(m)
                }
            },
            &cache_file,
            min,
        );
        let mc = sat_core::identical::mod_content(&table, &hashes, &prio);
        let (files, bytes, unknown) = mc.iter().fold((0u64, 0u64, 0u64), |a, c| {
            (
                a.0 + u64::from(c.equal_files),
                a.1 + c.equal_bytes,
                a.2 + u64::from(c.unknown_files),
            )
        });
        timings.insert("content_hashes", t.elapsed().as_millis());
        println!(
            "losing copies equal to the winner: {files} file(s), {}; not compared: {unknown} (BA2 textures, unreadable); {read} file(s) and archive(s) read now",
            human_bytes(bytes)
        );
        if has(args, "--identical") {
            let names = table.mod_names();
            for (m, c) in sat_core::identical::top(&mc).into_iter().take(20) {
                let full = table.summary(m).metrics.bytes;
                println!(
                    "  {:>10}  {:>6} file(s){}  {}",
                    human_bytes(c.equal_bytes),
                    c.equal_files,
                    if full > 0 && c.equal_bytes * 10 >= full * 9 {
                        "  (nearly the whole mod)"
                    } else {
                        ""
                    },
                    names[m.idx()]
                );
            }
        }
        Some(mc)
    };

    // @sat:cli.find
    if let Some(expr) = &find {
        let by_name: std::collections::HashMap<String, &Seen> =
            outcomes.iter().map(|o| (o.name.to_lowercase(), o)).collect();
        // a short answer by default: the count matters, the full list goes to a file
        let find_limit = match flag_value(args, "--limit").map(str::parse::<usize>) {
            Some(Ok(0)) => usize::MAX,
            Some(Ok(n)) => n,
            _ => 30,
        };
        let mut sep: Option<&str> = None;
        let mut hits = 0;
        for (pos, e) in list.entries.iter().enumerate() {
            if e.kind == sat_core::canon::EntryKind::Separator {
                sep = Some(e.name.trim_end_matches("_separator"));
                continue;
            }
            if e.kind != sat_core::canon::EntryKind::Mod {
                continue;
            }
            let seen = by_name.get(&e.name.to_lowercase());
            let tags = seen.filter(|_| rules.is_some()).map(|o| o.tags.join(", "));
            let view = sat_core::metrics::ModView {
                tags: tags.as_deref(),
                metrics: seen.and_then(|o| o.metrics.as_ref()),
                profile: index
                    .get(&e.name.to_lowercase())
                    .map(|&m| table.summary(m))
                    .filter(|s| !s.missing)
                    .map(|s| &s.profile),
                content: content
                    .as_ref()
                    .zip(index.get(&e.name.to_lowercase()))
                    .map(|(c, &m)| &c[m.idx()]),
                stat: index.get(&e.name.to_lowercase()).map(|&m| proj.stat(m)),
                name: &e.name,
                separator: sep,
                enabled: e.enabled,
                priority: pos as u32,
                source: sources.get(&e.name.to_lowercase()),
            };
            if expr.test(&|id| view.get(id)) {
                hits += 1;
                if hits > find_limit {
                    continue;
                }
                println!(
                    "{pos:>6}  {}{}",
                    e.name,
                    sep.map(|s| format!("   [{s}]")).unwrap_or_default()
                );
            }
        }
        if hits > find_limit {
            eprintln!(
                "… {} more (--limit N, or `> file.txt` with --limit 0 for all)",
                hits - find_limit
            );
        }
        eprintln!("find: {hits} mod(s) match `{}`", expr.source());
    }

    // @sat:cli.content
    // what the build is made of: package shapes, primary kinds, label frequencies (`@sat:profile`)
    if has(args, "--content") {
        use std::collections::BTreeMap as Map;
        let (mut shapes, mut kinds, mut labels) = (
            Map::<&str, usize>::new(),
            Map::<&str, usize>::new(),
            Map::<&str, usize>::new(),
        );
        let mut cargo: Vec<(&str, Vec<&str>)> = Vec::new();
        let mut junk: Vec<(&str, u32)> = Vec::new();
        for m in 0..table.mod_count() {
            let id = ModId::from_idx(m);
            let s = table.summary(id);
            if s.missing {
                continue;
            }
            let p = &s.profile;
            let name = table.mod_names()[m].as_str();
            *shapes.entry(p.package_shape()).or_default() += 1;
            *kinds.entry(p.primary_kind()).or_default() += 1;
            for l in p.labels() {
                *labels.entry(l).or_default() += 1;
            }
            let c = p.cargo_signals();
            if !c.is_empty() {
                cargo.push((name, c));
            }
            if p.junk > 0 {
                junk.push((name, p.junk));
            }
        }
        let show = |title: &str, m: &Map<&str, usize>| {
            let mut v: Vec<_> = m.iter().collect();
            v.sort_by_key(|x| std::cmp::Reverse(*x.1));
            println!("{title}:");
            for (k, n) in v {
                println!("  {n:>6}  {k}");
            }
        };
        show("package shapes", &shapes);
        show("primary kinds", &kinds);
        show("labels", &labels);
        println!("possible non-runtime cargo: {} mod(s)", cargo.len());
        for (name, c) in cargo.iter().take(20) {
            println!("  {name}   [{}]", c.join(", "));
        }
        junk.sort_by_key(|x| std::cmp::Reverse(x.1));
        println!("system junk: {} mod(s)", junk.len());
        for (name, n) in junk.iter().take(10) {
            println!("  {n:>6}  {name}");
        }
    }

    if let Some(r) = &rules {
        let mut count: std::collections::BTreeMap<&str, usize> = Default::default();
        let mut tagged = 0;
        for o in &outcomes {
            tagged += usize::from(!o.tags.is_empty());
            for t in &o.tags {
                *count.entry(t.as_str()).or_default() += 1;
            }
        }
        let mut top: Vec<(&str, usize)> = count.into_iter().collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        eprintln!(
            "tags ({}): {tagged} of {} mods tagged, {} distinct; top: {}",
            r.set().name,
            outcomes.len(),
            top.len(),
            top.iter()
                .take(15)
                .map(|(t, n)| format!("{t} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    // plugins: the manager's lists + versions across mods + mod priorities
    let t = Instant::now();
    let read = |f: &str| std::fs::read(inst.profile_dir().join(f)).unwrap_or_default();
    // @sat:cli.plugins-reconcile
    // the manager's lists are taken unless the user edited the project's since they were last seen
    let plugin_canon = if offline {
        project.plugins.clone()
    } else {
        let disk = PluginCanon {
            load_order: sat_mo2::profile::parse_loadorder(&read("loadorder.txt")),
            checked: sat_mo2::profile::parse_plugins(&read("plugins.txt"))
                .into_iter()
                .map(|l| (l.name, l.enabled))
                .collect(),
            blacklist: Vec::new(),
        };
        let (canon, r) =
            sat_core::plugin_edit::reconcile(&project.plugins, project.plugins_seen.as_ref(), &disk);
        if r.conflict {
            eprintln!(
                "plugins: changed both in the manager and in the project since the last run — the project's edit stands (the commit writes it)"
            );
        } else if r.pending {
            eprintln!("plugins: the project has edits of its own (the commit writes them)");
        }
        project.plugins_seen = Some(disk);
        canon
    };
    // the project keeps the plugin canon, so the plugin list works without the build
    project.plugins = plugin_canon.clone();
    let plugin_list = PluginList::compute(&plugin_versions, &plugin_canon, proj.priorities());
    timings.insert("plugins", t.elapsed().as_millis());
    let ps = plugin_list.stats;
    eprintln!(
        "plugins={} full={} light={} overflow={}/{} versioned={} missing-masters={} owner-off={} missing={}",
        ps.plugins,
        ps.active_full,
        ps.active_light,
        ps.overflow_full,
        ps.overflow_light,
        ps.versioned,
        ps.with_missing_masters,
        ps.owner_disabled,
        ps.missing
    );

    // @sat:cli.records
    // the record layer (`@sat:records`): loaded plugins' records, overrides as conflicts, chains by plugin stack
    if has(args, "--records") {
        if offline {
            eprintln!("--records reads the plugins: not with --offline");
        } else {
            let t = Instant::now();
            let loaded = sat_scan::records::loaded_plugins(
                &plugin_list,
                &table,
                |m| {
                    if m == OVERWRITE {
                        inst.overwrite_dir.clone()
                    } else {
                        inst.mods_dir.join(m)
                    }
                },
                &inst.game_path.join("Data"),
            );
            let (ix, unread, read) =
                sat_scan::records::index(&loaded, &inst.profile_dir().join("satellite").join("records"));
            let chains = sat_core::records::chains(&ix);
            let stats = sat_core::records::plugin_stats(&ix);
            timings.insert("records", t.elapsed().as_millis());
            println!(
                "records: {} in {} plugin(s), {} read now; overridden: {} (by one plugin: {}); conflicts (two or more plugins over a record): {} in {} chain(s); not read: {}",
                ix.records(),
                ix.plugins.len(),
                read,
                ix.overridden(),
                ix.overridden() - ix.conflicts(),
                ix.conflicts(),
                chains.iter().filter(|c| c.conflict).count(),
                unread.len()
            );
            for u in unread.iter().take(5) {
                println!("  not read: {u}");
            }
            let stack = |v: &[String]| {
                if v.len() <= 4 {
                    // ASCII arrows: PowerShell 5.1 garbles UTF-8 written to a file
                    v.join(" -> ")
                } else {
                    format!(
                        "{} -> {} -> ... -> {} ({} plugins)",
                        v[0],
                        v[1],
                        v[v.len() - 1],
                        v.len()
                    )
                }
            };
            let limit = match flag_value(args, "--limit").map(str::parse::<usize>) {
                Some(Ok(0)) => usize::MAX,
                Some(Ok(n)) => n,
                _ => 15,
            };
            for (title, conflict) in [
                (
                    "conflicts: two or more plugins over the same records, the last takes them whole",
                    true,
                ),
                (
                    "overrides: one plugin over the records of the plugin they come from",
                    false,
                ),
            ] {
                println!("{title}:");
                for c in chains.iter().filter(|c| c.conflict == conflict).take(limit) {
                    let mut sigs: Vec<_> = c.by_signature.iter().collect();
                    sigs.sort_by_key(|x| std::cmp::Reverse(*x.1));
                    let sigs: Vec<String> = sigs.iter().take(4).map(|(k, n)| format!("{k} {n}")).collect();
                    println!(
                        "  {:>7}  {}   [{}]{}",
                        c.records,
                        stack(&c.stack),
                        sigs.join(", "),
                        if c.equal_to_winner > 0 {
                            format!("  equal to the winner: {}", c.equal_to_winner)
                        } else {
                            String::new()
                        }
                    );
                    // records to look up in xEdit: origin plugin and FormID within it
                    println!("           e.g. {}", c.samples.join(", "));
                }
            }
            let mut by: Vec<(usize, &sat_core::records::PluginRecordStat)> =
                stats.iter().enumerate().collect();
            by.sort_by_key(|x| std::cmp::Reverse(x.1.loses));
            println!("plugins losing the most overrides to a later plugin (conflicts):");
            for (i, s) in by.iter().take(10).filter(|x| x.1.loses > 0) {
                println!(
                    "  {:>7}  {}   (overrides {}, wins {})",
                    s.loses, ix.plugins[*i], s.overrides, s.wins
                );
            }
            by.sort_by_key(|x| std::cmp::Reverse(x.1.unchanged));
            println!("unchanged overrides (equal to the record they override, ITM):");
            for (i, s) in by.iter().take(10).filter(|x| x.1.unchanged > 0) {
                println!("  {:>7}  {}", s.unchanged, ix.plugins[*i]);
            }
        }
    }

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
    let publish = seals.is_empty() && !has(args, "--no-save") && !offline;
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
        normalized: totals.normalized,
        contexts: totals.contexts,
        fully_overridden: totals.fully_overridden,
        plugins: ps,
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
        "mods={} cache={} missing={} files={} archive={} paths={} providers={} conflicts={} chains={} normalized={} contexts={} X={}",
        report.mods_scanned,
        report.from_cache,
        report.missing,
        report.files,
        report.archive_entries,
        report.paths,
        report.providers,
        report.conflict_paths,
        report.chains,
        report.normalized,
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
    match writer {
        Some(w) if publish => {
            let saved = w
                .finish(&project, sat_stll::file::keep_entities(&project))
                .map_err(|e| e.to_string())?;
            eprintln!(
                "project saved → {}: facts {} written, {} unchanged, {} dropped",
                project_path.display(),
                saved.facts_written,
                saved.facts_copied,
                saved.facts_dropped
            );
        }
        // not published: the half-written file goes away, the project on disk stays as it was
        _ => {}
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
            "compute#{round}: conflict_paths={} chains={} normalized={} contexts={} X={} in {:?}",
            tt.conflict_paths,
            tt.chains,
            tt.normalized,
            tt.contexts,
            tt.fully_overridden,
            t.elapsed()
        );
        // reorder deterministically, like the Go benchmark
        prio = Priorities((0..mods).map(|i| ((i * 7919 + round) % mods) as i32).collect());
    }
    // incremental: facts-only conflict index once, then updates touching a few mods
    let t = Instant::now();
    let index = std::sync::Arc::new(ConflictIndex::build(&table));
    eprintln!(
        "conflict index: contested={} in {:?}",
        index.contested_count(),
        t.elapsed()
    );
    let base: Vec<i32> = (0..mods as i32).collect();
    let t = Instant::now();
    let full = OverwriteProjection::compute_with(index.clone(), &table, Priorities(base.clone()));
    eprintln!("compute over index: {:?}", t.elapsed());
    // move mod 10 to the top: ranks of everything above it shift, relative order of others is kept
    let moved = 10usize;
    let mut order: Vec<usize> = (0..mods).collect();
    order.remove(moved);
    order.push(moved);
    let mut moved_prio = vec![0i32; mods];
    for (rank, &m) in order.iter().enumerate() {
        moved_prio[m] = rank as i32;
    }
    let mut inc = full;
    let t = Instant::now();
    inc.update_in_place(&table, Priorities(moved_prio.clone()), &[ModId::from_idx(moved)]);
    eprintln!("update in place: move one mod in {:?}", t.elapsed());
    let t = Instant::now();
    let _ = inc.totals();
    eprintln!(
        "contexts and normalized chains on first read after the move: {:?}",
        t.elapsed()
    );
    let mut off = moved_prio.clone();
    off[moved] = Priorities::INACTIVE;
    let mut tog = inc.clone();
    let t = Instant::now();
    tog.update_in_place(&table, Priorities(off), &[ModId::from_idx(moved)]);
    eprintln!("update in place: disable one mod in {:?}", t.elapsed());
    let t = Instant::now();
    let _copy = inc.update(&table, Priorities(moved_prio.clone()), &[ModId::from_idx(moved)]);
    eprintln!(
        "update on a copy (a reader holds the snapshot): {:?}",
        t.elapsed()
    );
    let check = OverwriteProjection::compute(&table, Priorities(moved_prio));
    if check.totals() != inc.totals() || check.stats() != inc.stats() {
        return Err("incremental update differs from full compute".into());
    }
    eprintln!(
        "after compute: rss={:?}MiB peak={:?}MiB threads={}",
        rss_mib(),
        peak_rss_mib(),
        rayon::current_num_threads()
    );
    Ok(())
}

/// `64K`, `1MB`, `2 GB`, `4096` → bytes (binary units).
fn parse_size(s: &str) -> Result<u64, String> {
    let t = s.trim().to_ascii_uppercase();
    let t = t.trim_end_matches('B').trim();
    let (num, mul) = match t.chars().last() {
        Some('K') => (&t[..t.len() - 1], 1u64 << 10),
        Some('M') => (&t[..t.len() - 1], 1 << 20),
        Some('G') => (&t[..t.len() - 1], 1 << 30),
        _ => (t, 1),
    };
    num.trim()
        .parse::<f64>()
        .map(|n| (n * mul as f64) as u64)
        .map_err(|_| format!("not a size: {s}"))
}

/// Bytes for people: `1.4 GB`.
fn human_bytes(b: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < units.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", units[u])
    }
}
