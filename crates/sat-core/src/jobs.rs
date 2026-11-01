//! Jobs: long work with cancel and generations; a stale result is never published. @sat:jobs
//!
//! Owns: tickets for running work by kind, superseding (a new job of a kind cancels the previous one), cancel
//! tokens, and the rule that decides whether a finished job may publish its result.
//! Not here: the work itself (scan `@sat:scan`, …), what publishing means (the shell hands results to the session).
//!
//! NG-D003 §4. The monolith's processors had `superseded` and its run kernel generations; a late result of an
//! older run could still overwrite a newer one when a path forgot the check. Here a ticket carries its generation,
//! and [`Jobs::publish`] is the one gate: it answers only for the latest ticket of the kind that was not
//! cancelled, and only once.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// What a job is (one running job per kind: a newer one supersedes it).
pub type JobKind = &'static str;

/// A running job's handle: the worker checks [`Ticket::cancelled`] between steps (or hands the token to the scan).
#[derive(Debug, Clone)]
pub struct Ticket {
    pub kind: JobKind,
    pub generation: u64,
    cancel: Arc<AtomicBool>,
}

impl Ticket {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// The token to hand to work that polls an `AtomicBool` itself (`ScanProgress::cancel`).
    pub fn token(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }
}

/// Why a result was not published.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// A newer job of the same kind started.
    Superseded,
    Cancelled,
    /// The result of this ticket was already published.
    AlreadyPublished,
}

#[derive(Debug)]
struct Running {
    generation: u64,
    cancel: Arc<AtomicBool>,
    published: bool,
}

// @sat:jobs.supervisor
/// The supervisor of jobs (one per app).
#[derive(Debug, Default)]
pub struct Jobs {
    next: u64,
    running: HashMap<JobKind, Running>,
}

impl Jobs {
    /// Starts a job of `kind`; a running job of the same kind is cancelled (superseded).
    pub fn start(&mut self, kind: JobKind) -> Ticket {
        self.next += 1;
        if let Some(old) = self.running.get(kind) {
            old.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.running.insert(
            kind,
            Running {
                generation: self.next,
                cancel: cancel.clone(),
                published: false,
            },
        );
        Ticket {
            kind,
            generation: self.next,
            cancel,
        }
    }

    /// Cancels the running job of `kind`, if any. Returns whether there was one.
    pub fn cancel(&mut self, kind: JobKind) -> bool {
        match self.running.get(kind) {
            Some(r) if !r.published => {
                r.cancel.store(true, Ordering::Relaxed);
                true
            }
            _ => false,
        }
    }

    /// Whether a job of `kind` is running (started, not cancelled, not published).
    pub fn busy(&self, kind: JobKind) -> bool {
        self.running
            .get(kind)
            .is_some_and(|r| !r.published && !r.cancel.load(Ordering::Relaxed))
    }

    // @sat:jobs.publish
    /// The gate before a result reaches the session: `Ok` only for the latest, not cancelled, not yet published
    /// ticket of its kind. Call it on the thread that owns the session, right before applying the result.
    pub fn publish(&mut self, t: &Ticket) -> Result<(), Refused> {
        let Some(r) = self.running.get_mut(t.kind) else {
            return Err(Refused::Superseded);
        };
        if r.generation != t.generation {
            return Err(Refused::Superseded);
        }
        if r.published {
            return Err(Refused::AlreadyPublished);
        }
        if r.cancel.load(Ordering::Relaxed) {
            return Err(Refused::Cancelled);
        }
        r.published = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_latest_live_result_is_published_once() {
        let mut j = Jobs::default();
        let first = j.start("scan");
        assert!(j.busy("scan"));
        // a new scan supersedes the first: its token is set, its result is refused
        let second = j.start("scan");
        assert!(first.cancelled() && !second.cancelled());
        assert_eq!(j.publish(&first), Err(Refused::Superseded));
        // another kind runs alongside
        let tags = j.start("tags");
        assert_eq!(j.publish(&second), Ok(()));
        assert_eq!(j.publish(&second), Err(Refused::AlreadyPublished));
        assert!(!j.busy("scan") && j.busy("tags"));
        // a cancelled job publishes nothing
        assert!(j.cancel("tags"));
        assert!(tags.cancelled());
        assert_eq!(j.publish(&tags), Err(Refused::Cancelled));
        assert!(!j.cancel("scan"), "nothing running");
    }

    #[test]
    fn a_worker_thread_sees_the_cancel() {
        let mut j = Jobs::default();
        let t = j.start("scan");
        let token = t.token();
        let worker = std::thread::spawn(move || {
            let mut steps = 0;
            while !token.load(Ordering::Relaxed) && steps < 1_000_000 {
                steps += 1;
                std::thread::yield_now();
            }
            steps
        });
        j.cancel("scan");
        assert!(worker.join().unwrap() < 1_000_000);
        assert_eq!(j.publish(&t), Err(Refused::Cancelled));
    }
}
