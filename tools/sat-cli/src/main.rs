//! `sat-cli` — developer tool for NG-D001 evidence.
//!
//! ```text
//! sat-cli analyze <instance> | --mods DIR --profiles DIR [--overwrite DIR]  [--profile NAME] [--listed-only] [--io-threads N] [--cache DIR] [--force] [--records] [--report FILE]
//! sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--profile NAME] [--report FILE]
//! sat-cli bench-synth [--mods N] [--files F] [--rounds R]
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use sat_core::canon::{EntryKind, Modlist};
use sat_core::facts::{LooseDir, LooseFile, ModFacts};
use sat_core::ids::ModId;
use sat_core::overwrite::{OverwriteProjection, Priorities};
use sat_core::sync::donor;
use sat_core::table::ProviderTable;
use sat_mo2::{Instance, profile};
use sat_scan::{ScanJob, ScanOptions, ScanProgress};
use serde::Serialize;

const OVERWRITE: &str = "Overwrite";

/// Millions of small allocations (path strings, per-dir vectors) dominate table builds; the system allocator on
/// Windows is markedly slower at that than mimalloc.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("analyze") => analyze(&args[1..]),
        Some("donor") => donor(&args[1..]),
        Some("bench-synth") => bench_synth(&args[1..]),
        _ => Err("usage: sat-cli analyze <instance> | --mods DIR --profiles DIR [--overwrite DIR]  [--profile NAME] [--listed-only] [--io-threads N] [--cache DIR] [--force] [--records] [--report FILE]\n       sat-cli donor <instance> --donor-mods DIR [--donor-modlist FILE] [--profile NAME] [--report FILE]\n       sat-cli bench-synth [--mods N] [--files F] [--rounds R]".into()),
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
fn rss_mib() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|kb| kb / 1024)
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
}

#[derive(Serialize)]
struct Report {
    instance: InstanceInfo,
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
}

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
    let list = read_modlist(&inst.profile_dir().join("modlist.txt"))?;

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
        let out = sat_scan::scan(&jobs, &opts, &progress);
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        out
    });
    timings.insert("scan", t.elapsed().as_millis());

    let ms = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed) / 1_000_000;
    let n = |c: &std::sync::atomic::AtomicU64| c.load(std::sync::atomic::Ordering::Relaxed);
    let mut slowest: Vec<(String, u128, u32, u32)> = outcomes
        .iter()
        .filter(|o| !o.from_cache)
        .map(|o| {
            (
                o.facts.name.clone(),
                o.elapsed.as_millis(),
                o.facts.metrics.files,
                o.facts.metrics.archive_files,
            )
        })
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
    };
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

    let from_cache = outcomes.iter().filter(|o| o.from_cache).count();
    let missing = outcomes
        .iter()
        .filter(|o| o.facts.missing && o.facts.name != OVERWRITE)
        .count();
    let files: u64 = outcomes.iter().map(|o| u64::from(o.facts.metrics.files)).sum();
    let archive_entries: u64 = outcomes
        .iter()
        .map(|o| u64::from(o.facts.metrics.archive_files))
        .sum();
    let facts: Vec<ModFacts> = outcomes.into_iter().map(|o| o.facts).collect();

    let t = Instant::now();
    let table = ProviderTable::build(&facts);
    timings.insert("table_build", t.elapsed().as_millis());
    drop(facts);
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
    let report = Report {
        instance: info,
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
