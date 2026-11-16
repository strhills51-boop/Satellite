//! The window's runs over the build: the app's requests run as jobs. @sat:devserver.pass
//!
//! Each run is `@sat:home.run` on a thread of its own.
//!
//! Owns: starting a run the app asked for
//! (`@sat:app.pass`) on a thread of its own as a job of kind `run` (`@sat:jobs`: a newer run supersedes, cancel
//! between batches, a late result is never published), its progress, and publishing a finished run into the app:
//! new facts as one change, the sync records with the decisions the user made meanwhile, sources, plugin lists.
//! With `--write` it also saves the project when the app asks (`@sat:home.project.save`: a file another program
//! wrote meanwhile is not overwritten unless forced) and a full run writes its facts and the project like
//! `sat-cli analyze`; without it nothing is written.
//! Not here: the run (`@sat:home.run`), the app's truth (`@sat:app.project`), the file (`@sat:stll.file`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use sat_app::App;
use sat_app::commit::{CommitResult, CommitState};
use sat_app::pass::{PassMode, PassProgress, PassResult};
use sat_core::jobs::{Jobs, Ticket};
use sat_core::plugins::{PluginCanon, PluginVersions};
use sat_core::project::Project;
use sat_core::session::Session;
use sat_core::sync::records::{Reconcile, Status, SyncRecord};
use sat_core::table::ProviderTable;
use sat_home::project::{SaveError, Stamp};
use sat_home::run::{RunOptions, Stage, Totals};
use sat_mo2::Instance;
use sat_scan::ScanProgress;
use sat_stll::Stll;

const RUN: sat_core::jobs::JobKind = "run";

/// The build as the shell holds it.
#[derive(Debug)]
pub struct Home {
    inst: Instance,
    path: PathBuf,
    /// `--write`: saves and full runs write the project file.
    write: bool,
    /// The project file as this window last read or wrote it.
    stamp: Option<Stamp>,
    jobs: Jobs,
    running: Option<Running>,
    /// A commit on its own thread.
    committing: Option<Receiver<CommitResult>>,
}

#[derive(Debug)]
struct Running {
    ticket: Ticket,
    mode: PassMode,
    progress: Arc<ScanProgress>,
    stage: Stage,
    started: Instant,
    /// The plugin canon the run started from: an edit in the window meanwhile wins over the run's lists.
    plugins_at_start: PluginCanon,
    rx: Receiver<Result<Ran, String>>,
}

/// What a run thread hands back.
#[derive(Debug)]
struct Ran {
    /// The project as the run left it (signatures, records, sources, plugin lists, fulfilled intents).
    project: Project,
    sync: Reconcile,
    facts: Option<Facts>,
    /// The project file was written by the run: the project as written.
    saved: Option<Project>,
    /// How long the run itself took (the result waits for the next request to be published).
    took: std::time::Duration,
}

#[derive(Debug)]
struct Facts {
    table: Arc<ProviderTable>,
    versions: Arc<PluginVersions>,
    mods: usize,
    from_cache: usize,
    unlisted: usize,
    seals: Vec<String>,
    degraded: Vec<String>,
}

impl Home {
    pub fn new(inst: Instance, path: PathBuf, write: bool, stamp: Option<Stamp>) -> Self {
        Self {
            inst,
            path,
            write,
            stamp,
            jobs: Jobs::default(),
            running: None,
            committing: None,
        }
    }

    // @sat:devserver.save
    fn save(&mut self, app: &mut App, force: bool) {
        let Some(p) = app.project() else {
            return;
        };
        match sat_home::project::save_checked(&self.path, &p, self.stamp, force) {
            Ok(st) => {
                self.stamp = Some(st);
                eprintln!("project saved → {}", self.path.display());
                app.save_done(p);
            }
            Err(e) => {
                eprintln!("project not saved: {e}");
                app.save_failed(e.to_string(), e == SaveError::ChangedOutside);
            }
        }
    }

    // @sat:devserver.pass.tick
    /// Called around every request: a cancel, a new request, the progress, a finished run, a commit. Returns a
    /// finished commit that wrote the build: the shell opens the build again (the run is the commit's check).
    pub fn tick(&mut self, app: &mut App) -> Option<CommitResult> {
        if let Some(rx) = &self.committing {
            return match rx.try_recv() {
                Ok(r) => {
                    self.committing = None;
                    self.stamp = sat_home::project::stamp_of(&self.path);
                    if r.state == CommitState::Committed {
                        Some(r)
                    } else {
                        app.commit_finished(r);
                        None
                    }
                }
                // one writer: nothing else starts while the commit runs
                Err(_) => None,
            };
        }
        if let Some(confirmed) = app.take_commit_request() {
            self.commit(app, confirmed);
            return None;
        }
        if app.take_pass_cancel() && self.jobs.cancel(RUN) {
            eprintln!("run: cancel asked");
        }
        if let Some(force) = app.take_save_request() {
            self.save(app, force);
        }
        if let Some(mode) = app.take_pass_request() {
            self.start(app, mode);
        }
        self.poll(app);
        None
    }

    // @sat:devserver.commit
    /// Starts the commit of the window's project on its own thread: a running run is superseded (its result is
    /// not published), the gates are checked there (`commit_gates`), then the transaction (`@sat:home.commit`).
    fn commit(&mut self, app: &mut App, confirmed: Vec<String>) {
        if !self.write {
            app.commit_finished(CommitResult {
                state: CommitState::Refused,
                error: Some("the window is a sandbox (start it with --write)".into()),
                ..Default::default()
            });
            return;
        }
        let Some(project) = app.project() else {
            return;
        };
        self.jobs.cancel(RUN);
        self.running = None;
        app.commit_started();
        let (tx, rx) = channel();
        let (inst, path, stamp) = (self.inst.clone(), self.path.clone(), self.stamp);
        eprintln!("commit: started");
        std::thread::spawn(move || {
            let r = commit_gates(&inst, &path, &project, &confirmed, stamp);
            eprintln!(
                "commit: {:?}{}",
                r.state,
                r.error.as_deref().map(|e| format!(" — {e}")).unwrap_or_default()
            );
            let _ = tx.send(r);
        });
        self.committing = Some(rx);
    }

    fn poll(&mut self, app: &mut App) {
        let Some(r) = &self.running else {
            return;
        };
        match r.rx.try_recv() {
            Ok(out) => {
                let r = self.running.take().expect("running");
                self.finish(app, r, out);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                let (done, total, cached, files) = r.progress.snapshot();
                let stage = r.stage.lock().map(|g| *g).unwrap_or("scan");
                app.pass_progress(
                    r.ticket.generation,
                    PassProgress {
                        stage: stage.into(),
                        done,
                        total,
                        cached,
                        files,
                        elapsed_ms: r.started.elapsed().as_millis() as u64,
                    },
                );
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                let r = self.running.take().expect("running");
                self.finish(app, r, Err("the run stopped without a result".into()));
            }
        }
    }

    fn start(&mut self, app: &mut App, mode: PassMode) {
        // a newer run supersedes the running one: its result will not be published
        let ticket = self.jobs.start(RUN);
        // the run reconciles the canon and the decisions the user has now
        let Some(mut project) = app.project() else {
            return;
        };
        let plugins_at_start = project.plugins.clone();
        let inst = self.inst.clone();
        let path = self.path.clone();
        // a full run writes facts and the project only over the file this window knows
        let save = self.write && mode == PassMode::Full && sat_home::project::stamp_of(&path) == self.stamp;
        if self.write && mode == PassMode::Full && !save {
            app.save_failed(
                "the run did not write the project: the file changed outside the window since it was read"
                    .into(),
                true,
            );
        }
        let progress = Arc::new(ScanProgress {
            cancel: ticket.token(),
            ..Default::default()
        });
        let stage: Stage = Arc::new(std::sync::Mutex::new("inventory"));
        let (tx, rx) = channel();
        let (p, st) = (progress.clone(), stage.clone());
        std::thread::spawn(move || {
            let t = Instant::now();
            let out = match mode {
                PassMode::Surface => {
                    if let Ok(mut g) = st.lock() {
                        *g = "sync";
                    }
                    let sync = sat_home::run::surface(&inst, &mut project);
                    Ok(Ran {
                        project,
                        sync,
                        facts: None,
                        saved: None,
                        took: Default::default(),
                    })
                }
                PassMode::Full => full(&inst, path, project, &p, st, save),
            };
            let _ = tx.send(out.map(|mut r| {
                r.took = t.elapsed();
                r
            }));
        });
        eprintln!("run: {mode:?} started (generation {})", ticket.generation);
        app.pass_started(mode, ticket.generation);
        self.running = Some(Running {
            ticket,
            mode,
            progress,
            stage,
            started: Instant::now(),
            plugins_at_start,
            rx,
        });
    }

    fn finish(&mut self, app: &mut App, r: Running, out: Result<Ran, String>) {
        let elapsed_ms = r.started.elapsed().as_millis() as u64;
        let mut result = PassResult {
            mode: Some(r.mode),
            generation: r.ticket.generation,
            elapsed_ms,
            ..Default::default()
        };
        // the one gate: a cancelled or superseded run publishes nothing
        if self.jobs.publish(&r.ticket).is_err() || r.ticket.cancelled() {
            result.cancelled = true;
            eprintln!(
                "run: generation {} not published (cancelled or superseded)",
                r.ticket.generation
            );
            app.pass_finished(result);
            return;
        }
        let ran = match out {
            Ok(x) => {
                result.elapsed_ms = x.took.as_millis() as u64;
                x
            }
            Err(e) => {
                eprintln!("run: failed: {e}");
                result.error = Some(e);
                app.pass_finished(result);
                return;
            }
        };
        if let Some(f) = &ran.facts {
            result.mods = f.mods;
            result.from_cache = f.from_cache;
            result.unlisted = f.unlisted;
            result.seals = f.seals.clone();
            result.degraded = f.degraded.clone();
            if !f.seals.is_empty() {
                eprintln!("run: seals failed, nothing published: {}", f.seals.join("; "));
                app.pass_finished(result);
                return;
            }
        }
        // what the user decided while the run went on stays: a record with the same fingerprint is the same
        // mismatch (`@sat:sync.records.reconcile`)
        let records = merge_decisions(ran.sync.records, app.sync_records());
        result.records = records.len();
        result.pending = records.iter().filter(|x| x.status == Status::Pending).count();
        let mut p = ran.project;
        let run_plugins = p.plugins.clone();
        // the canon and the window's other truth are the app's; the run brought signatures, sources, the
        // manager's plugin lists and fulfilled intents
        app.store_project(&mut p);
        p.sync = records;
        if p.plugins == r.plugins_at_start {
            p.plugins = run_plugins;
        } else {
            // the window's plugin edit made during the run wins; the manager's lists the run saw are remembered
            eprintln!("run: the plugin lists were edited in the window during the run — the edit stands");
        }
        if let Some(saved) = ran.saved {
            self.stamp = sat_home::project::stamp_of(&self.path);
            eprintln!("run: project and facts saved → {}", self.path.display());
            app.project_saved(saved);
        }
        if let Some(f) = ran.facts {
            app.set_facts(f.table, f.versions);
            crate::start_readers(&self.inst, app);
        }
        for e in app.load_project(&p, sat_home::subfolders(&self.inst.mods_dir)) {
            eprintln!("{e}");
        }
        eprintln!(
            "run: {:?} published, ran {:.1} s — {} record(s), {} pending",
            r.mode,
            result.elapsed_ms as f64 / 1000.0,
            result.records,
            result.pending
        );
        app.pass_finished(result);
    }
}

/// The run's records with the decisions now in the window (by fingerprint).
fn merge_decisions(mut fresh: Vec<SyncRecord>, now: &[SyncRecord]) -> Vec<SyncRecord> {
    for r in &mut fresh {
        if let Some(n) = now.iter().find(|n| n.fingerprint == r.fingerprint) {
            r.status = n.status;
            r.resolution = n.resolution;
        }
    }
    fresh
}

// @sat:devserver.pass.full
/// The full run on its own thread: the scan, the run's project, conflicts on a session of the new facts (the same
/// code the window uses), seals. Nothing is written (sandbox).
fn full(
    inst: &Instance,
    path: PathBuf,
    mut project: Project,
    progress: &ScanProgress,
    stage: Stage,
    save: bool,
) -> Result<Ran, String> {
    // the stored facts of unchanged mods are copied from the file as it is
    let mut stll = if save && path.exists() {
        Stll::open(&path).map_err(|e| e.to_string())?.0
    } else {
        Stll::new(path)
    };
    let inv = sat_home::run::inventory(inst, &project.canon);
    let jobs = sat_home::run::jobs(inst, &project.canon, &inv.unlisted, true);
    let mut o = RunOptions::for_build(inst);
    o.save_facts = save;
    o.tag_rules = Some(Arc::new(sat_core::tag_rules::Compiled::builtin()));
    o.stage = Some(stage.clone());
    let (scanned, pending) =
        sat_home::run::scan(inst, &mut stll, &mut project, &jobs, &o, progress, &mut |_| {})?;
    if progress.cancel.load(std::sync::atomic::Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    if let Ok(mut g) = stage.lock() {
        *g = "conflicts";
    }
    let table = Arc::new(scanned.table);
    let mut session = Session::new(project.book().0, table.clone());
    let totals = session.overwrite().totals();
    if let Ok(mut g) = stage.lock() {
        *g = "seals";
    }
    // a run cancelled or superseded meanwhile writes nothing
    let save = save && !progress.cancel.load(std::sync::atomic::Ordering::Relaxed);
    let published = pending.publish(
        &project,
        Totals {
            chains: totals.chains,
            contexts: totals.contexts,
            conflict_paths: totals.conflict_paths,
        },
        save,
    )?;
    let saved = published.saved.is_some().then(|| project.clone());
    Ok(Ran {
        facts: Some(Facts {
            table,
            versions: Arc::new(scanned.plugin_versions),
            mods: scanned.mods.len(),
            from_cache: scanned.mods.iter().filter(|m| m.from_cache).count(),
            unlisted: inv.unlisted.len(),
            seals: published.seals,
            degraded: published.degraded,
        }),
        project,
        sync: scanned.sync,
        saved,
        took: Default::default(),
    })
}

// @sat:devserver.plan
/// The full plan of the build for the window (`@sat:app.sync.full`): the same function `sat-cli commit` runs.
#[derive(Debug)]
pub struct HomePlan {
    pub inst: Instance,
    pub layout: sat_mo2::commit::Layout,
}

impl sat_app::PlanSource for HomePlan {
    fn full_plan(
        &self,
        project: &Project,
        confirmed: &[String],
    ) -> (sat_core::sync::plan::Plan, sat_app::PlanExtras) {
        let confirmed: Vec<&str> = confirmed.iter().map(String::as_str).collect();
        let f = sat_home::plan::full(&self.inst, &self.layout, project, &confirmed);
        let extras = sat_app::PlanExtras {
            plugins_written: f.plugins.as_ref().is_some_and(|w| w.edited),
            plugins_packed: f.plugins.as_ref().map(|w| w.packed.clone()).unwrap_or_default(),
            factories: f
                .built
                .iter()
                .map(|(i, _)| project.factories[*i].output.clone())
                .collect(),
            sealed: f
                .sealed
                .iter()
                .map(|&i| project.factories[i].output.clone())
                .collect(),
            patches: f.patched.as_ref().map(Option::is_some),
            notes: f.notes,
        };
        (f.plan, extras)
    }
}

/// The commit's gates and the commit (on the commit's thread): the project file is as this window knows it, the
/// mods folder is as the records describe it (a surface run now, `@sat:home.commit.fresh`), the full plan is not
/// blocked — then the transaction with its backup, journal and rollback.
fn commit_gates(
    inst: &Instance,
    path: &std::path::Path,
    project: &Project,
    confirmed: &[String],
    stamp: Option<Stamp>,
) -> CommitResult {
    let refused = |e: String| CommitResult {
        state: CommitState::Refused,
        error: Some(e),
        ..Default::default()
    };
    if sat_home::project::stamp_of(path) != stamp {
        return refused(
            "the project file changed outside the window since it was read — reopen the window".into(),
        );
    }
    let stale = sat_home::commit::stale(inst, project);
    if !stale.is_empty() {
        return refused(format!(
            "the mods folder changed since the last run ({}) — run the surface again and look at the records",
            stale.join(", ")
        ));
    }
    let layout = sat_mo2::commit::Layout::of(inst);
    let confirmed: Vec<&str> = confirmed.iter().map(String::as_str).collect();
    let full = sat_home::plan::full(inst, &layout, project, &confirmed);
    if full.plan.blocked() {
        return refused(format!(
            "the plan is blocked ({} blocker(s))",
            full.plan.blockers.len()
        ));
    }
    let stll = if path.exists() {
        match Stll::open(path) {
            Ok((s, _)) => s,
            Err(e) => return refused(e.to_string()),
        }
    } else {
        Stll::new(path)
    };
    let done = match sat_home::commit::run(&layout, stll, project, &full, &sat_mo2::commit::stamp_now()) {
        Ok(d) => d,
        Err(e) => return refused(e),
    };
    let mut r = CommitResult {
        version_dir: Some(done.report.version_dir.display().to_string()),
        steps: done.report.steps.len(),
        ..Default::default()
    };
    match done.report.outcome {
        sat_mo2::commit::Outcome::Committed => r.state = CommitState::Committed,
        sat_mo2::commit::Outcome::RolledBack { error } => {
            r.state = CommitState::RolledBack;
            r.error = Some(error);
        }
        sat_mo2::commit::Outcome::RollbackIncomplete { error, left } => {
            r.state = CommitState::Incomplete;
            r.error = Some(error);
            r.left = left;
        }
        sat_mo2::commit::Outcome::Refused { error } => {
            r.state = CommitState::Refused;
            r.error = Some(error);
        }
    }
    r.error = r
        .error
        .or_else(|| (!done.notes.is_empty()).then(|| done.notes.join("; ")));
    r
}
