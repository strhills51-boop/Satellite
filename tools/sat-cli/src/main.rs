//! `sat-cli` — developer tool for NG-D001 evidence.
//!
//! ```text
//! sat-cli analyze <instance> [--extra DIR]... [--cache DIR] [--force] [--records] [--report FILE]
//! sat-cli bench-synth [--mods N] [--files F] [--rounds R]
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use sat_core::canon::Modlist;
use sat_core::facts::{LooseDir, LooseFile, ModFacts};
use sat_core::ids::ModId;
use sat_core::overwrite::{OverwriteProjection, Priorities};
use sat_core::table::ProviderTable;
use sat_mo2::{Instance, profile};
use sat_scan::{ScanJob, ScanOptions, ScanProgress};
use serde::Serialize;

const OVERWRITE: &str = "Overwrite";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let res = match args.first().map(String::as_str) {
        Some("analyze") => analyze(&args[1..]),
        Some("bench-synth") => bench_synth(&args[1..]),
        _ => Err("usage: sat-cli analyze <instance> [--extra DIR]... [--cache DIR] [--force] [--records] [--report FILE]\n       sat-cli bench-synth [--mods N] [--files F] [--rounds R]".into()),
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
struct Report {
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
    peak_rss_mib: Option<u64>,
    rss_after_mib: Option<u64>,
}

fn analyze(args: &[String]) -> Result<(), String> {
    let path = args
        .first()
        .filter(|a| !a.starts_with("--"))
        .ok_or("missing <instance>")?;
    let mut inst = Instance::load(path).map_err(|e| e.to_string())?;
    inst.extra_mod_dirs = flag_values(args, "--extra")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let data =
        std::fs::read(inst.profile_dir().join("modlist.txt")).map_err(|e| format!("modlist.txt: {e}"))?;
    let (list, _style) = profile::parse_modlist(&data);

    let mut jobs: Vec<ScanJob> = list
        .entries
        .iter()
        .filter(|e| e.is_mod())
        .map(|e| ScanJob {
            name: e.name.clone(),
            path: inst.resolve_mod_dir(&e.name),
        })
        .collect();
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
        ..Default::default()
    };
    let mut timings = BTreeMap::new();
    let t = Instant::now();
    let outcomes = sat_scan::scan(&jobs, &opts, &ScanProgress::default());
    timings.insert("scan", t.elapsed().as_millis());

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
