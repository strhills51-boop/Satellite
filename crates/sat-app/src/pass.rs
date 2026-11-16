//! The run (пробег) as the window sees it: asked by a command, run by the shell, shown by a query. @sat:app.pass
//!
//! Owns: the run's state in the app — a request the shell takes (`StartPass`, `CancelPass`), the progress and the
//! result the shell hands back, and taking a finished run's new facts in (`App::set_facts`: the table replaced as
//! one change, caches that point into the old table dropped). The app does not touch the disk (ADR 0004): the run
//! itself is the shell's job over `@sat:home.run`, gated by `@sat:jobs`.
//! Not here: the run (`@sat:home.run`), its generation and superseding (`@sat:jobs`), the sync records' merge
//! (`@sat:sync.records.reconcile`).

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::App;

/// How deep a run goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PassMode {
    /// Folder signatures and the sync records only: seconds; the facts stay (monolith «Поверхность»).
    Surface,
    /// Everything: the scan (cached), facts, records, sources, plugin lists, conflicts (monolith «Бездна»).
    Full,
}

/// Where the run is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PassState {
    /// No run since the window opened (the opening run is not shown as one).
    #[default]
    Idle,
    /// Asked; the shell has not taken it yet.
    Requested,
    Running,
    /// Finished and published.
    Done,
    /// Finished without publishing (an error or failed seals): the window keeps the previous facts.
    Failed,
    Cancelled,
}

/// A running run's progress, as the shell reads it off the scan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassProgress {
    /// The stage now: `inventory`, `scan`, `sync`, `sources`, `plugins`, `conflicts`, `seals`.
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub cached: usize,
    pub files: u64,
    pub elapsed_ms: u64,
}

/// What a finished run did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassResult {
    pub mode: Option<PassMode>,
    pub generation: u64,
    /// Mods scanned (Overwrite included) and how many came from the cache.
    pub mods: usize,
    pub from_cache: usize,
    /// Folders the canon does not list (scanned as inactive).
    pub unlisted: usize,
    /// Sync records after the run and how many wait for a decision.
    pub records: usize,
    pub pending: usize,
    /// Seal violations: not empty — nothing was published.
    pub seals: Vec<String>,
    /// Mods with degraded facts (`name: why`).
    pub degraded: Vec<String>,
    pub elapsed_ms: u64,
    /// Why the run did not finish.
    pub error: Option<String>,
    pub cancelled: bool,
}

/// The run as the window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PassView {
    /// The shell can run (a build is open; a project alone cannot be run over).
    pub available: bool,
    pub state: PassState,
    pub mode: Option<PassMode>,
    /// The generation of the running (or last) run.
    pub generation: u64,
    pub progress: Option<PassProgress>,
    pub last: Option<PassResult>,
}

#[derive(Debug, Default)]
pub(crate) struct PassSlot {
    pub view: PassView,
    request: Option<PassMode>,
    cancel: bool,
}

impl App {
    /// The shell can run over the build (it takes requests).
    pub fn enable_passes(&mut self) {
        self.pass.view.available = true;
    }

    pub(crate) fn request_pass(&mut self, mode: PassMode) -> bool {
        if !self.pass.view.available {
            return false;
        }
        // a newer request supersedes a running run (the shell cancels it when it starts the new one)
        self.pass.request = Some(mode);
        self.pass.view.state = PassState::Requested;
        self.pass.view.mode = Some(mode);
        true
    }

    pub(crate) fn request_cancel(&mut self) -> bool {
        let running = matches!(self.pass.view.state, PassState::Running | PassState::Requested);
        if running {
            self.pass.cancel = true;
            if self.pass.view.state == PassState::Requested {
                self.pass.request = None;
                self.pass.view.state = PassState::Cancelled;
            }
        }
        running
    }

    /// The shell takes a requested run.
    pub fn take_pass_request(&mut self) -> Option<PassMode> {
        self.pass.request.take()
    }

    /// The shell takes a cancel request.
    pub fn take_pass_cancel(&mut self) -> bool {
        std::mem::take(&mut self.pass.cancel)
    }

    /// The shell started a run of this generation.
    pub fn pass_started(&mut self, mode: PassMode, generation: u64) {
        let v = &mut self.pass.view;
        v.state = PassState::Running;
        v.mode = Some(mode);
        v.generation = generation;
        v.progress = Some(PassProgress::default());
    }

    /// Progress of the running run (ignored for another generation).
    pub fn pass_progress(&mut self, generation: u64, p: PassProgress) {
        if self.pass.view.generation == generation && self.pass.view.state == PassState::Running {
            self.pass.view.progress = Some(p);
        }
    }

    /// A run ended (published or not). The shell has already handed over its facts and records when published.
    pub fn pass_finished(&mut self, r: PassResult) {
        let v = &mut self.pass.view;
        if r.generation != v.generation {
            return;
        }
        v.state = if r.cancelled {
            PassState::Cancelled
        } else if r.error.is_some() || !r.seals.is_empty() {
            PassState::Failed
        } else {
            PassState::Done
        };
        v.progress = None;
        v.last = Some(r);
        self.revision += 1;
    }

    pub(crate) fn pass_view(&self) -> PassView {
        self.pass.view.clone()
    }

    // @sat:app.pass.facts
    /// New facts of a finished run: the table and the plugin versions are replaced as one change (ADR 0010 law
    /// 11: a whole new input), caches that index into the old table are dropped — content hashes until the shell
    /// hands new ones, the record layer marked pending until the shell's new report.
    pub fn set_facts(
        &mut self,
        table: Arc<sat_core::table::ProviderTable>,
        versions: Arc<sat_core::plugins::PluginVersions>,
    ) {
        self.session.set_facts(table);
        self.session.set_plugin_facts(versions);
        self.content_hashes = Arc::new(Default::default());
        self.content_rev += 1;
        self.mod_content = None;
        self.vfs = None;
        self.heads = None;
        self.records_pending();
        // layouts measured the old badges and counters
        self.flows.clear();
        self.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::app_of;
    use crate::{Command, Query, Reply};

    fn view(a: &mut App) -> PassView {
        match a.query(Query::Pass).unwrap() {
            Reply::Pass { pass } => pass,
            _ => unreachable!(),
        }
    }

    #[test]
    fn requests_progress_and_results() {
        let mut a = app_of(["A", "B"]);
        // no shell that runs: the request is nothing
        a.command(Command::StartPass { mode: PassMode::Full }).unwrap();
        assert_eq!(a.take_pass_request(), None);
        assert_eq!(view(&mut a).state, PassState::Idle);

        a.enable_passes();
        a.command(Command::StartPass {
            mode: PassMode::Surface,
        })
        .unwrap();
        assert_eq!(view(&mut a).state, PassState::Requested);
        assert_eq!(a.take_pass_request(), Some(PassMode::Surface));
        assert_eq!(a.take_pass_request(), None, "taken once");
        a.pass_started(PassMode::Surface, 1);
        a.pass_progress(
            1,
            PassProgress {
                stage: "sync".into(),
                done: 3,
                total: 9,
                ..Default::default()
            },
        );
        // progress of another generation is not shown
        a.pass_progress(
            7,
            PassProgress {
                stage: "scan".into(),
                ..Default::default()
            },
        );
        assert_eq!(view(&mut a).progress.unwrap().stage, "sync");

        // a cancel is handed to the shell; an old generation's result is ignored
        a.command(Command::CancelPass).unwrap();
        assert!(a.take_pass_cancel());
        assert!(!a.take_pass_cancel());
        a.pass_finished(PassResult {
            generation: 0,
            ..Default::default()
        });
        assert_eq!(view(&mut a).state, PassState::Running);
        a.pass_finished(PassResult {
            generation: 1,
            cancelled: true,
            ..Default::default()
        });
        assert_eq!(view(&mut a).state, PassState::Cancelled);

        a.command(Command::StartPass { mode: PassMode::Full }).unwrap();
        a.pass_started(PassMode::Full, 2);
        a.pass_finished(PassResult {
            generation: 2,
            seals: vec!["x".into()],
            ..Default::default()
        });
        assert_eq!(
            view(&mut a).state,
            PassState::Failed,
            "failed seals publish nothing"
        );
        a.pass_started(PassMode::Full, 3);
        a.pass_finished(PassResult {
            generation: 3,
            records: 1,
            ..Default::default()
        });
        let v = view(&mut a);
        assert_eq!((v.state, v.last.unwrap().records), (PassState::Done, 1));
    }

    #[test]
    fn new_facts_replace_the_table() {
        let mut a = app_of(["A", "B"]);
        let before = a.session().overwrite().totals().conflict_paths;
        assert_eq!(before, 1, "A and B share textures/a.dds");
        // B no longer ships the shared file
        let facts = vec![
            sat_core::facts::ModFacts {
                name: "A".into(),
                ..Default::default()
            },
            sat_core::facts::ModFacts {
                name: "B".into(),
                ..Default::default()
            },
        ];
        let table = Arc::new(sat_core::table::ProviderTable::build(&facts));
        a.set_facts(table, Arc::new(Default::default()));
        assert_eq!(a.session().overwrite().totals().conflict_paths, 0);
    }
}
