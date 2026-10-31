//! Parallel scanner: mod folders → [`sat_core::facts::ModFacts`].
//!
//! - Work is parallel across mods **and** inside large mods (subdirectories are walked in parallel),
//!   so one 200k-file mod does not serialize the tail of a run.
//! - Output is deterministic: entries are sorted by name and subtree results are joined in order.
//! - Cold tier: results are cached per mod, keyed by a cheap signature (names/sizes/mtimes to depth 2 +
//!   scan options + cache version). Unchanged mods are not walked again.

mod cache;
mod signature;
mod walk;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use rayon::prelude::*;

use sat_core::facts::ModFacts;

pub use cache::CACHE_VERSION;
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
    pub from_cache: bool,
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
    pub cached: AtomicUsize,
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

/// Scans in batches of `batch` jobs (parallel inside a batch) and hands every outcome to `sink` **in job order**.
/// Only one batch of facts is alive at a time, so the caller can fold outcomes into a table and drop them.
pub fn scan_batched(
    jobs: &[ScanJob],
    opts: &ScanOptions,
    progress: &ScanProgress,
    batch: usize,
    mut sink: impl FnMut(usize, ScanOutcome),
) {
    progress.total.store(jobs.len(), Ordering::Relaxed);
    let run = |chunk: &[ScanJob]| scan_on_current_pool(chunk, opts, progress);
    let pool = pool(opts);
    for (ci, chunk) in jobs.chunks(batch.max(1)).enumerate() {
        let outcomes = match &pool {
            Some(p) => p.install(|| run(chunk)),
            None => run(chunk),
        };
        for (i, o) in outcomes.into_iter().enumerate() {
            sink(ci * batch.max(1) + i, o);
        }
    }
}

fn scan_on_current_pool(jobs: &[ScanJob], opts: &ScanOptions, progress: &ScanProgress) -> Vec<ScanOutcome> {
    jobs.par_iter()
        .map(|job| {
            let out = scan_one(job, opts, progress);
            progress.done.fetch_add(1, Ordering::Relaxed);
            out
        })
        .collect()
}

fn scan_one(job: &ScanJob, opts: &ScanOptions, progress: &ScanProgress) -> ScanOutcome {
    let started = std::time::Instant::now();
    if !job.path.is_dir() {
        return ScanOutcome {
            facts: ModFacts::missing(&job.name),
            from_cache: false,
            signature: 0,
            elapsed: started.elapsed(),
        };
    }
    let t = std::time::Instant::now();
    let sig = signature(&job.path, opts);
    add_ns(&progress.signature_ns, t);
    let t = std::time::Instant::now();
    let cached = if opts.force {
        None
    } else {
        opts.cache_dir.as_ref().and_then(|dir| {
            cache::load(dir, &job.name, sig).or_else(|| {
                // Adopt an entry written under the previous signature scheme, then re-key it.
                let facts = cache::load(dir, &job.name, signature::signature_v1(&job.path, opts))?;
                cache::store(dir, &facts, sig);
                Some(facts)
            })
        })
    };
    add_ns(&progress.cache_load_ns, t);
    if let Some(facts) = cached {
        progress.cached.fetch_add(1, Ordering::Relaxed);
        progress
            .files
            .fetch_add(u64::from(facts.metrics.files), Ordering::Relaxed);
        return ScanOutcome {
            facts,
            from_cache: true,
            signature: sig,
            elapsed: started.elapsed(),
        };
    }
    let t = std::time::Instant::now();
    let facts = walk::walk_mod(&job.name, &job.path, opts, progress);
    add_ns(&progress.walk_ns, t);
    if let Some(dir) = &opts.cache_dir
        && facts.error.is_none()
    {
        cache::store(dir, &facts, sig);
    }
    ScanOutcome {
        facts,
        from_cache: false,
        signature: sig,
        elapsed: started.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopts_cache_written_under_v1_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let m = tmp.path().join("M");
        std::fs::create_dir_all(m.join("textures")).unwrap();
        std::fs::write(m.join("textures/a.txt"), b"x").unwrap();
        let cache = tmp.path().join("cache");
        let opts = ScanOptions {
            cache_dir: Some(cache.clone()),
            ..Default::default()
        };
        let job = ScanJob {
            name: "M".into(),
            path: m.clone(),
        };
        let first = scan_one(&job, &opts, &ScanProgress::default());
        // Pretend an older build wrote the entry.
        cache::store(&cache, &first.facts, signature::signature_v1(&m, &opts));
        let adopted = scan_one(&job, &opts, &ScanProgress::default());
        assert!(adopted.from_cache, "v1 entry adopted");
        // Re-keyed: found directly under the current signature.
        assert!(cache::load(&cache, "M", signature(&m, &opts)).is_some());
    }
}
