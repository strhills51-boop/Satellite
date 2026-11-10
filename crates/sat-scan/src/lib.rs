//! Parallel scanner: mod folders → [`sat_core::facts::ModFacts`]. @sat:scan
//!
//! - Work is parallel across mods **and** inside large mods (subdirectories are walked in parallel),
//!   so one 200k-file mod does not serialize the tail of a run.
//! - Output is deterministic: entries are sorted by name and subtree results are joined in order.
//! - Every file is an observation for the registered analyzers ([`analyzer`]): metrics, DDS VRAM, plugins,
//!   archives (a source expander whose entries are observed too).
//! - Cold tier: results are cached per mod with a content signature (names/sizes/mtimes to depth 2) and the
//!   analyzer stamps. Unchanged mods are reused; when only some analyzers changed, the mod is re-analyzed from its
//!   cached index by those analyzers (route `indexed`, no walk); otherwise it is walked (route `scan`).
//!
//! Owns: the scan entry points, options, progress, the I/O pool and route selection per mod.

pub mod analyzer;
mod cache;
pub mod content;
mod signature;
mod walk;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use rayon::prelude::*;

use sat_core::facts::ModFacts;

pub use analyzer::{Registry, Route, Stamp};
pub use cache::CACHE_FORMAT;
pub use signature::signature;

/// One folder to scan.
#[derive(Debug, Clone)]
pub struct ScanJob {
    pub name: String,
    pub path: PathBuf,
}

/// What to read during the walk.
#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub read_dds_headers: bool,
    pub archives: bool,
    pub plugins: bool,
    pub record_scan: bool,
    /// Root-level file names (lowercase) excluded from the virtual file system, e.g. `meta.ini`.
    pub ignore_root_files: Vec<String>,
    pub cache_dir: Option<PathBuf>,
    /// Ignore cached results.
    pub force: bool,
    /// Worker threads for the scan. The scan waits on the disk, not the CPU: an SSD (especially over USB)
    /// serves dozens of requests at once, so this is much larger than the core count. `0` = default.
    pub io_threads: usize,
}

/// Default I/O concurrency: 8 per core, at least 32.
pub fn default_io_threads() -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    (cores * 8).max(32)
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            read_dds_headers: true,
            archives: true,
            plugins: true,
            record_scan: false,
            ignore_root_files: vec!["meta.ini".into(), "desktop.ini".into(), "thumbs.db".into()],
            cache_dir: None,
            force: false,
            io_threads: 0,
        }
    }
}

/// Result for one job.
#[derive(Debug)]
pub struct ScanOutcome {
    pub facts: ModFacts,
    /// Not walked this run (`reuse` or `indexed`).
    pub from_cache: bool,
    pub route: Route,
    /// Failures of optional and degraded analyzers in this run (mandatory ones are `facts.error`).
    pub issues: Vec<String>,
    pub signature: u64,
    pub elapsed: Duration,
}

/// Live counters, readable from another thread while a scan runs.
///
/// The `*_ns` fields are thread time summed over all workers (not wall time): they show where the scan
/// spends its effort, e.g. opening DDS files vs. reading archive indexes vs. computing signatures.
#[derive(Debug, Default)]
pub struct ScanProgress {
    pub total: AtomicUsize,
    pub done: AtomicUsize,
    /// Mods not walked (reuse + indexed).
    pub cached: AtomicUsize,
    /// Mods re-analyzed from their cached index.
    pub indexed: AtomicUsize,
    pub files: AtomicU64,
    pub signature_ns: AtomicU64,
    pub cache_load_ns: AtomicU64,
    pub dds_reads: AtomicU64,
    pub dds_ns: AtomicU64,
    pub archive_reads: AtomicU64,
    pub archive_ns: AtomicU64,
    pub plugin_reads: AtomicU64,
    pub plugin_ns: AtomicU64,
    pub walk_ns: AtomicU64,
    /// Set from another thread to stop the scan: the batch in flight finishes (its outcomes still reach the sink,
    /// in order), no further batch starts. See [`Completion`].
    pub cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// How a batched scan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Done,
    /// Stopped by [`ScanProgress::cancel`]; this many jobs reached the sink.
    Cancelled {
        delivered: usize,
    },
}

pub(crate) fn add_ns(counter: &AtomicU64, since: std::time::Instant) {
    counter.fetch_add(since.elapsed().as_nanos() as u64, Ordering::Relaxed);
}

impl ScanProgress {
    pub fn snapshot(&self) -> (usize, usize, usize, u64) {
        (
            self.done.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
            self.cached.load(Ordering::Relaxed),
            self.files.load(Ordering::Relaxed),
        )
    }
}

/// Stack of scan threads. The pool steals work inside joins, so one thread can run a mod's directory walk on top of
/// another's; on deep trees the 2 MiB default overflowed on the owner's build. Reserved, not committed, memory.
const SCAN_STACK_BYTES: usize = 32 << 20;

// @sat:scan.io-pool
fn pool(opts: &ScanOptions) -> Option<rayon::ThreadPool> {
    let threads = if opts.io_threads == 0 {
        default_io_threads()
    } else {
        opts.io_threads
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .stack_size(SCAN_STACK_BYTES)
        .thread_name(|i| format!("sat-scan-{i}"))
        .build()
        .ok()
}

/// Scans all jobs on a dedicated I/O pool ([`ScanOptions::io_threads`]). Results are returned in job order.
///
/// Holds every result in memory; for large builds use [`scan_batched`].
pub fn scan(jobs: &[ScanJob], opts: &ScanOptions, progress: &ScanProgress) -> Vec<ScanOutcome> {
    let mut out = Vec::with_capacity(jobs.len());
    scan_batched(jobs, opts, progress, jobs.len().max(1), |_, o| out.push(o));
    out
}

// @sat:scan.batched
/// Scans in batches of `batch` jobs (parallel inside a batch) and hands every outcome to `sink` **in job order**.
/// Only one batch of facts is alive at a time, so the caller can fold outcomes into a table and drop them.
pub fn scan_batched(
    jobs: &[ScanJob],
    opts: &ScanOptions,
    progress: &ScanProgress,
    batch: usize,
    sink: impl FnMut(usize, ScanOutcome),
) -> Completion {
    scan_batched_with(&Registry::builtin(opts), jobs, opts, progress, batch, sink)
}

/// [`scan_batched`] with an explicit analyzer registry (extensions, tests).
pub fn scan_batched_with(
    reg: &Registry,
    jobs: &[ScanJob],
    opts: &ScanOptions,
    progress: &ScanProgress,
    batch: usize,
    mut sink: impl FnMut(usize, ScanOutcome),
) -> Completion {
    progress.total.store(jobs.len(), Ordering::Relaxed);
    let mut delivered = 0;
    let ctx = walk::Ctx { reg, opts, progress };
    let run = |chunk: &[ScanJob]| scan_on_current_pool(chunk, &ctx);
    let pool = pool(opts);
    for (ci, chunk) in jobs.chunks(batch.max(1)).enumerate() {
        // @sat:scan.cancel
        if progress.cancel.load(Ordering::Relaxed) {
            return Completion::Cancelled { delivered };
        }
        let outcomes = match &pool {
            Some(p) => p.install(|| run(chunk)),
            None => run(chunk),
        };
        for (i, o) in outcomes.into_iter().enumerate() {
            sink(ci * batch.max(1) + i, o);
            delivered += 1;
        }
    }
    Completion::Done
}

fn scan_on_current_pool(jobs: &[ScanJob], ctx: &walk::Ctx<'_>) -> Vec<ScanOutcome> {
    jobs.par_iter()
        .map(|job| {
            let out = scan_one(job, ctx);
            ctx.progress.done.fetch_add(1, Ordering::Relaxed);
            out
        })
        .collect()
}

/// Cached facts and the stamps they were computed with, if the content did not change.
fn cached_for(job: &ScanJob, sig: u64, ctx: &walk::Ctx<'_>) -> Option<(ModFacts, Vec<Stamp>, bool)> {
    let dir = ctx.opts.cache_dir.as_ref()?;
    match cache::load(dir, &job.name)? {
        cache::Cached::Current {
            signature,
            stamps,
            facts,
        } => (signature == sig).then_some((facts, stamps, false)),
        cache::Cached::Legacy { signature, facts } => {
            // The first format folded the options into the signature and had every analyzer at version 1.
            let legacy = [signature::signature_v2, signature::signature_v1];
            legacy
                .iter()
                .any(|f| f(&job.path, ctx.opts) == signature)
                .then(|| {
                    let stamps = ctx
                        .reg
                        .stamps()
                        .into_iter()
                        .map(|s| Stamp { version: 1, ..s })
                        .collect();
                    (facts, stamps, true)
                })
        }
    }
}

fn scan_one(job: &ScanJob, ctx: &walk::Ctx<'_>) -> ScanOutcome {
    let (opts, progress) = (ctx.opts, ctx.progress);
    let started = std::time::Instant::now();
    if !job.path.is_dir() {
        return ScanOutcome {
            facts: ModFacts::missing(&job.name),
            from_cache: false,
            route: Route::Scan,
            issues: Vec::new(),
            signature: 0,
            elapsed: started.elapsed(),
        };
    }
    let t = std::time::Instant::now();
    let sig = signature(&job.path);
    add_ns(&progress.signature_ns, t);
    let t = std::time::Instant::now();
    let cached = if opts.force {
        None
    } else {
        cached_for(job, sig, ctx)
    };
    add_ns(&progress.cache_load_ns, t);
    let stamps = ctx.reg.stamps();
    let store = |facts: &ModFacts| {
        if let Some(dir) = &opts.cache_dir
            && facts.error.is_none()
        {
            cache::store(dir, facts, sig, &stamps);
        }
    };
    let route = cached
        .as_ref()
        .map_or(Route::Scan, |(_, s, _)| analyzer::plan(ctx.reg, s));
    let (facts, issues) = match (route.clone(), cached) {
        (Route::Reuse, Some((facts, _, legacy))) => {
            if legacy {
                store(&facts); // adopted from the first format: re-key
            }
            (facts, Vec::new())
        }
        (Route::Indexed { rerun, cleared }, Some((mut facts, _, _))) => {
            let t = std::time::Instant::now();
            let issues = walk::reanalyze(&mut facts, &job.path, &rerun, &cleared, ctx);
            add_ns(&progress.walk_ns, t);
            progress.indexed.fetch_add(1, Ordering::Relaxed);
            store(&facts);
            (facts, issues.other)
        }
        _ => {
            let t = std::time::Instant::now();
            let (facts, issues) = walk::walk_mod(&job.name, &job.path, ctx);
            add_ns(&progress.walk_ns, t);
            store(&facts);
            (facts, issues.other)
        }
    };
    let from_cache = route != Route::Scan;
    if from_cache {
        progress.cached.fetch_add(1, Ordering::Relaxed);
        if route == Route::Reuse {
            progress
                .files
                .fetch_add(u64::from(facts.metrics.files), Ordering::Relaxed);
        }
    }
    ScanOutcome {
        facts,
        from_cache,
        route,
        issues,
        signature: sig,
        elapsed: started.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dds(w: u32, h: u32) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(b"DDS ");
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&1u32.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(b"DXT1");
        b
    }

    fn fixture() -> (tempfile::TempDir, ScanJob) {
        let tmp = tempfile::tempdir().unwrap();
        let m = tmp.path().join("M");
        std::fs::create_dir_all(m.join("textures/sky")).unwrap();
        std::fs::write(m.join("textures/sky/a.dds"), dds(256, 256)).unwrap();
        std::fs::write(m.join("textures/b.txt"), b"x").unwrap();
        std::fs::write(m.join("meta.ini"), b"[General]").unwrap();
        let job = ScanJob {
            name: "M".into(),
            path: m,
        };
        (tmp, job)
    }

    fn run(job: &ScanJob, opts: &ScanOptions) -> (ScanOutcome, ScanProgress) {
        let progress = ScanProgress::default();
        let reg = Registry::builtin(opts);
        let ctx = walk::Ctx {
            reg: &reg,
            opts,
            progress: &progress,
        };
        (scan_one(job, &ctx), progress)
    }

    #[test]
    fn routes_reuse_indexed_scan() {
        let (tmp, job) = fixture();
        let cache = tmp.path().join("cache");
        let base = ScanOptions {
            cache_dir: Some(cache),
            read_dds_headers: false,
            ..Default::default()
        };
        let (first, _) = run(&job, &base);
        assert_eq!(first.route, Route::Scan);
        assert_eq!(first.facts.metrics.vram_bytes, 0);
        let (again, p) = run(&job, &base);
        assert_eq!(again.route, Route::Reuse);
        assert_eq!(p.dds_reads.load(Ordering::Relaxed), 0);

        // enabling DDS headers re-analyzes the cached index: no walk, only that analyzer
        let with_dds = ScanOptions {
            read_dds_headers: true,
            ..base.clone()
        };
        let (idx, p) = run(&job, &with_dds);
        assert_eq!(
            idx.route,
            Route::Indexed {
                rerun: vec!["dds-vram".into()],
                cleared: vec![]
            }
        );
        assert_eq!(p.files.load(Ordering::Relaxed), 0, "nothing walked");
        assert_eq!(p.dds_reads.load(Ordering::Relaxed), 1);
        let fresh = run(
            &job,
            &ScanOptions {
                force: true,
                ..with_dds.clone()
            },
        )
        .0;
        assert_eq!(idx.facts, fresh.facts, "indexed result equals a fresh walk");
        assert!(idx.facts.metrics.vram_bytes > 0);

        // disabling it again clears its facts without a walk
        let (back, _) = run(&job, &base);
        assert_eq!(
            back.route,
            Route::Indexed {
                rerun: vec![],
                cleared: vec!["dds-vram".into()]
            }
        );
        assert_eq!(back.facts, first.facts);

        // the ignore list belongs to the structural analyzer: a change walks again
        let ignore = ScanOptions {
            ignore_root_files: vec![],
            ..base.clone()
        };
        assert_eq!(run(&job, &ignore).0.route, Route::Scan);
    }

    #[test]
    fn adopts_entries_of_the_first_cache_format() {
        let (tmp, job) = fixture();
        let cache = tmp.path().join("cache");
        let opts = ScanOptions {
            cache_dir: Some(cache.clone()),
            ..Default::default()
        };
        let fresh = run(
            &job,
            &ScanOptions {
                cache_dir: None,
                ..opts.clone()
            },
        )
        .0;
        for legacy_sig in [
            signature::signature_v2(&job.path, &opts),
            signature::signature_v1(&job.path, &opts),
        ] {
            cache::store_legacy(&cache, &fresh.facts, legacy_sig);
            let (adopted, p) = run(&job, &opts);
            assert_eq!(adopted.route, Route::Reuse, "legacy entry adopted");
            assert_eq!(
                p.files.load(Ordering::Relaxed),
                adopted.facts.metrics.files as u64
            );
            assert_eq!(adopted.facts, fresh.facts);
            // re-keyed into the current format
            assert!(matches!(
                cache::load(&cache, "M"),
                Some(cache::Cached::Current { .. })
            ));
        }
    }

    #[test]
    fn registry_rejects_bad_contracts() {
        let mut r = Registry::builtin(&ScanOptions::default());
        let ids: Vec<&str> = r.contracts().iter().map(|c| c.id).collect();
        assert_eq!(ids, ["metrics", "dds-vram", "plugins", "archives"]);
        struct Dup;
        impl analyzer::Analyzer for Dup {
            fn contract(&self) -> analyzer::Contract {
                analyzer::Contract {
                    id: "plugins",
                    version: 1,
                    class: analyzer::ExecutionClass::StreamConsumer,
                    failure: analyzer::FailurePolicy::Optional,
                    structural: false,
                    reads_virtual: false,
                }
            }
            fn accepts(&self, _: &analyzer::Observation<'_>) -> bool {
                false
            }
            fn consume(
                &self,
                _: &analyzer::Observation<'_>,
                _: &mut analyzer::Part<'_>,
                _: &ScanProgress,
            ) -> Result<(), String> {
                Ok(())
            }
            fn clear(&self, _: &mut ModFacts) {}
        }
        assert!(r.register(Box::new(Dup)).is_err());
    }

    #[test]
    fn a_cancelled_scan_stops_between_batches_and_keeps_order() {
        let tmp = tempfile::tempdir().unwrap();
        let jobs: Vec<ScanJob> = (0..6)
            .map(|i| {
                let p = tmp.path().join(format!("M{i}"));
                std::fs::create_dir_all(&p).unwrap();
                std::fs::write(p.join("a.txt"), b"x").unwrap();
                ScanJob {
                    name: format!("M{i}"),
                    path: p,
                }
            })
            .collect();
        let opts = ScanOptions::default();
        let progress = ScanProgress::default();
        let mut seen = Vec::new();
        let end = scan_batched(&jobs, &opts, &progress, 2, |i, _| {
            seen.push(i);
            if i == 1 {
                // the user cancels while the first batch is being delivered
                progress.cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(end, Completion::Cancelled { delivered: 2 });
        assert_eq!(seen, [0, 1]);
        progress.cancel.store(false, Ordering::Relaxed);
        assert_eq!(
            scan_batched(&jobs, &opts, &progress, 4, |_, _| {}),
            Completion::Done
        );
    }
}
