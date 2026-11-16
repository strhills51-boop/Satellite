//! The window's runs over the build: the app's requests run as jobs. @sat:devserver.pass
//!
//! Each run is `@sat:home.run` on a thread of its own.
//!
//! Owns: starting a run the app asked for
//! (`@sat:app.pass`) on a thread of its own as a job of kind `run` (`@sat:jobs`: a newer run supersedes, cancel
//! between batches, a late result is never published), its progress, and publishing a finished run into the app:
//! new facts as one change, the sync records with the decisions the user made meanwhile, sources, plugin lists.
//! Not here: the run (`@sat:home.run`), the app's truth (`@sat:app.project`), saving (the sandbox writes nothing;
//! ADR 0019 step 4 adds `--write`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use sat_app::App;
use sat_app::pass::{PassMode, PassProgress, PassResult};
use sat_core::jobs::{Jobs, Ticket};
use sat_core::plugins::{PluginCanon, PluginVersions};
use sat_core::project::Project;
use sat_core::session::Session;
use sat_core::sync::records::{Reconcile, Status, SyncRecord};
use sat_core::table::ProviderTable;
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
    jobs: Jobs,
    running: Option<Running>,
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
    pub fn new(inst: Instance, path: PathBuf) -> Self {
        Self {
            inst,
            path,
            jobs: Jobs::default(),
            running: None,
        }
    }

    // @sat:devserver.pass.tick
    /// Called around every request: a cancel, a new request, the progress, a finished run.
    pub fn tick(&mut self, app: &mut App) {
        if app.take_pass_cancel() && self.jobs.cancel(RUN) {
            eprintln!("run: cancel asked");
        }
        if let Some(mode) = app.take_pass_request() {
            self.start(app, mode);
        }
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
                        took: Default::default(),
                    })
                }
                PassMode::Full => full(&inst, path, project, &p, st),
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
) -> Result<Ran, String> {
    let mut stll = Stll::new(path);
    let inv = sat_home::run::inventory(inst, &project.canon);
    let jobs = sat_home::run::jobs(inst, &project.canon, &inv.unlisted, true);
    let mut o = RunOptions::for_build(inst);
    o.save_facts = false;
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
    let published = pending.publish(
        &project,
        Totals {
            chains: totals.chains,
            contexts: totals.contexts,
            conflict_paths: totals.conflict_paths,
        },
        false,
    )?;
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
