//! The run over a build: inventory, scan, facts, sync records, sources, plugins, seals. @sat:home.run
//!
//! Owns: one run of a build for every shell (the CLI's `analyze`, the window) — what is scanned (the canon's mods,
//! folders the canon does not list, Overwrite), the facts the project keeps (`@sat:stll.file.facts`), the
//! reconcile of the mods folder with the project (`@sat:project.observe`), where mods come from (`meta.ini`), the
//! plugin lists taken from the manager unless the project edited its own, and the seals before anything is
//! published (`@sat:run.seal`).
//! Not here: the scan itself (`@sat:scan`), conflicts (the caller computes them its own way — the CLI a
//! projection, the window its session — and hands the totals to [`Pending::publish`]), reports.
//!
//! Two steps, because the conflicts sit between them: [`scan`] → the caller's conflicts → [`Pending::publish`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use sat_core::canon::Modlist;
use sat_core::entity::EntityId;
use sat_core::facts::ModFacts;
use sat_core::plugins::{PluginCanon, PluginVersions};
use sat_core::project::Project;
use sat_core::run::{RunState, RunStatus, RunSummary, StagedMod, StagedRoute, seal};
use sat_core::session::OVERWRITE;
use sat_core::sync::names::is_separator_folder;
use sat_core::sync::records::{DiskFolder, Reconcile};
use sat_core::table::{ProviderTable, TableBuilder};
use sat_mo2::{Instance, profile};
use sat_scan::{ScanJob, ScanOptions, ScanOutcome, ScanProgress};
use sat_stll::Stll;
use sat_stll::file::{Saved, Writer};

use crate::subfolders;

/// A mods root as the inventory saw it.
#[derive(Debug, Clone)]
pub struct Root {
    pub path: PathBuf,
    pub exists: bool,
    pub folders: usize,
    pub separator_folders: usize,
}

/// What the canon says vs. what is on disk, before the scan.
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub roots: Vec<Root>,
    /// Mod folders on disk (not separators) the canon does not list.
    pub unlisted: Vec<(String, PathBuf)>,
}

// @sat:home.run.inventory
/// The mods folder against the canon.
pub fn inventory(inst: &Instance, canon: &Modlist) -> Inventory {
    let listed: HashSet<String> = canon.entries.iter().map(|e| e.name.to_lowercase()).collect();
    let mut inv = Inventory::default();
    let mut seen = HashSet::new();
    for root in [inst.mods_dir.as_path()] {
        let names = subfolders(root);
        inv.roots.push(Root {
            path: root.to_path_buf(),
            exists: root.is_dir(),
            folders: names.len(),
            separator_folders: names.iter().filter(|n| is_separator_folder(n)).count(),
        });
        for n in names {
            let key = n.to_lowercase();
            if !is_separator_folder(&n) && !listed.contains(&key) && seen.insert(key) {
                inv.unlisted.push((n.clone(), root.join(&n)));
            }
        }
    }
    inv
}

/// How to run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Scan folders the canon does not list too (as inactive; ADR 0006: the run covers every folder).
    pub all_folders: bool,
    /// The build is not here: the project's facts stand in for the scan.
    pub offline: bool,
    /// Write the facts into the project file as they come (published only if the seals pass).
    pub save_facts: bool,
    pub scan: ScanOptions,
    /// Mods scanned per batch before their facts are folded into the table and dropped.
    pub batch: usize,
    /// Tag rules the table applies (found tags in the window).
    pub tag_rules: Option<Arc<sat_core::tag_rules::Compiled>>,
}

impl RunOptions {
    /// The defaults of a build on this machine: every folder, the profile's cache, facts saved.
    pub fn for_build(inst: &Instance) -> Self {
        Self {
            all_folders: true,
            offline: false,
            save_facts: true,
            scan: ScanOptions {
                cache_dir: Some(inst.profile_dir().join("satellite").join("cache")),
                ..Default::default()
            },
            batch: 256,
            tag_rules: None,
        }
    }
}

/// One mod of the run, kept after its facts are folded into the table.
#[derive(Debug, Clone)]
pub struct ModRun {
    pub name: String,
    pub missing: bool,
    pub from_cache: bool,
    pub route: StagedRoute,
    pub error: Option<String>,
    pub signature: u64,
    pub files: u32,
    pub archive_files: u32,
    pub elapsed: Duration,
}

/// What the plugin lists of the run are.
#[derive(Debug, Clone, Copy, Default)]
pub struct PluginNote {
    /// Changed both in the manager and in the project since the last run: the project's edit stands.
    pub conflict: bool,
    /// The project has edits of its own (the commit writes them).
    pub pending: bool,
}

/// What the run found; the table goes to the caller's conflicts.
#[derive(Debug)]
pub struct Scanned {
    pub table: ProviderTable,
    pub plugin_versions: PluginVersions,
    pub mods: Vec<ModRun>,
    /// Failures of optional/degraded analyzers (first 200, plus facts not saved) and their total.
    pub issues: Vec<String>,
    pub issue_count: usize,
    /// The mods folder against the project; offline — the records of the last real run.
    pub sync: Reconcile,
    /// Mods with a source in `meta.ini` read now and how long it took (`None` offline).
    pub sources: Option<Duration>,
    pub plugins: PluginNote,
}

/// The rest of a run, until its seals are checked.
#[derive(Debug)]
pub struct Pending<'a> {
    run: RunState,
    expected: Vec<String>,
    staged: Vec<StagedMod>,
    writer: Option<Writer<'a>>,
}

/// The canon's mods, then unlisted folders (when asked), then Overwrite.
pub fn jobs(
    inst: &Instance,
    canon: &Modlist,
    unlisted: &[(String, PathBuf)],
    all_folders: bool,
) -> Vec<ScanJob> {
    let mut jobs: Vec<ScanJob> = canon
        .entries
        .iter()
        .filter(|e| e.is_mod())
        .map(|e| ScanJob {
            name: e.name.clone(),
            path: inst.mod_dir(&e.name),
        })
        .collect();
    if all_folders {
        jobs.extend(unlisted.iter().map(|(name, path)| ScanJob {
            name: name.clone(),
            path: path.clone(),
        }));
    }
    jobs.push(ScanJob {
        name: OVERWRITE.into(),
        path: inst.overwrite_dir.clone(),
    });
    jobs
}

/// Entity ids of the project by lower-case name, Overwrite included: the keys of the facts it keeps.
fn fact_ids(project: &Project) -> HashMap<String, EntityId> {
    project
        .entities
        .iter()
        .map(|e| (e.name.to_lowercase(), e.id.clone()))
        .chain(std::iter::once((
            OVERWRITE.to_lowercase(),
            EntityId("overwrite".into()),
        )))
        .collect()
}

// @sat:home.run.scan
/// Scans the build (or reads the project's facts offline) and brings the project up to date with what was seen:
/// sync records, sources, plugin lists. `on_outcome` sees every mod's facts before they are folded into the table
/// (a shell keeping its own report columns). Nothing is published until [`Pending::publish`].
pub fn scan<'a>(
    inst: &Instance,
    stll: &'a mut Stll,
    project: &mut Project,
    jobs: &[ScanJob],
    o: &RunOptions,
    progress: &ScanProgress,
    on_outcome: &mut dyn FnMut(&ScanOutcome),
) -> Result<(Scanned, Pending<'a>), String> {
    let ids = fact_ids(project);
    let fact_id = |name: &str| ids.get(&name.to_lowercase()).cloned();
    let planned: HashSet<String> = project
        .planned()
        .into_iter()
        .map(|p| p.name.to_lowercase())
        .collect();
    // offline, the facts are read from the same file through a handle of its own
    let mut source = if o.offline && stll.path().exists() {
        Some(Stll::open(stll.path()).map_err(|e| e.to_string())?.0)
    } else {
        None
    };
    // the project file being written while the scan runs: facts go in as they come, unchanged ones are copied
    let mut writer = if o.save_facts && !o.offline {
        Some(stll.begin().map_err(|e| e.to_string())?)
    } else {
        None
    };
    let mut run = RunState::default();
    run.go(RunStatus::Running, "scan").map_err(|e| e.to_string())?;
    let mut builder = match &o.tag_rules {
        Some(r) => TableBuilder::new().with_tag_rules(r.clone()),
        None => TableBuilder::new(),
    };
    let mut versions = PluginVersions::default();
    let mut mods = Vec::with_capacity(jobs.len());
    let (mut issues, mut issue_count) = (Vec::<String>::new(), 0usize);
    // Facts are folded into the table batch by batch and dropped: peak memory = table + one batch.
    let mut sink = |mut out: ScanOutcome| {
        // a planned mod has no folder yet: its facts come from its archive (`sat-cli plan-install`)
        if out.facts.missing
            && planned.contains(&out.facts.name.to_lowercase())
            && let Some(eid) = fact_id(&out.facts.name)
            && let Some((sig, f)) = writer.as_mut().and_then(|w| w.stored_facts(&eid.0))
        {
            out.facts = f;
            out.signature = sig;
        }
        // @sat:home.run.facts
        // the project keeps what the scan saw, by entity id, so it works without the build
        if !out.facts.missing
            && let Some(eid) = fact_id(&out.facts.name)
            && let Some(w) = writer.as_mut()
            && let Err(e) = w.put_facts(&eid.0, out.signature, &out.facts)
            && issues.len() < 200
        {
            issues.push(format!("{}: project facts not saved: {e}", out.facts.name));
        }
        on_outcome(&out);
        let id = builder.add(&out.facts);
        versions.add(id, &out.facts);
        issue_count += out.issues.len();
        for i in &out.issues {
            if issues.len() < 200 {
                issues.push(format!("{}: {i}", out.facts.name));
            }
        }
        mods.push(ModRun {
            name: out.facts.name.clone(),
            missing: out.facts.missing,
            from_cache: out.from_cache,
            route: match out.route {
                sat_scan::Route::Scan => StagedRoute::Scan,
                sat_scan::Route::Indexed { .. } => StagedRoute::Indexed,
                sat_scan::Route::Reuse => StagedRoute::Reuse,
            },
            error: out.facts.error.clone(),
            signature: out.signature,
            files: out.facts.metrics.files,
            archive_files: out.facts.metrics.archive_files,
            elapsed: out.elapsed,
        });
    };
    if o.offline {
        // the build is not here: the project's facts stand in for the scan
        for job in jobs {
            let stored = fact_id(&job.name).and_then(|eid| source.as_mut()?.facts(&eid.0));
            let (signature, facts) = stored.unwrap_or_else(|| (0, ModFacts::missing(job.name.clone())));
            progress.done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            sink(ScanOutcome {
                facts,
                from_cache: true,
                route: sat_scan::Route::Reuse,
                issues: Vec::new(),
                signature,
                elapsed: Duration::ZERO,
            });
        }
    } else {
        sat_scan::scan_batched(jobs, &o.scan, progress, o.batch, |_, out| sink(out));
    }
    drop(sink);

    // @sat:home.run.sync
    // the mods folder against the project; offline there is no mods folder: the records of the last real run stand
    let sync = if o.offline {
        Reconcile {
            records: project.sync.clone(),
            matched: Vec::new(),
        }
    } else {
        let sig: HashMap<String, u64> = mods
            .iter()
            .filter(|m| !m.missing && m.name != OVERWRITE)
            .map(|m| (m.name.to_lowercase(), m.signature))
            .collect();
        let disk: Vec<DiskFolder> = subfolders(&inst.mods_dir)
            .into_iter()
            .map(|name| DiskFolder {
                signature: sig.get(&name.to_lowercase()).copied(),
                name,
            })
            .collect();
        project.observe(&disk)
    };

    // @sat:home.run.sources
    // where each mod comes from, as MO2 recorded it (`meta.ini`): read anew each run, offline kept from the project
    let sources = (!o.offline).then(|| {
        use rayon::prelude::*;
        let t = std::time::Instant::now();
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
        t.elapsed()
    });

    // @sat:home.run.plugins
    // the manager's lists are taken unless the user edited the project's since they were last seen
    let mut plugins = PluginNote::default();
    if !o.offline {
        let read = |f: &str| std::fs::read(inst.profile_dir().join(f)).unwrap_or_default();
        let disk = PluginCanon {
            load_order: profile::parse_loadorder(&read("loadorder.txt")),
            checked: profile::parse_plugins(&read("plugins.txt"))
                .into_iter()
                .map(|l| (l.name, l.enabled))
                .collect(),
            blacklist: Vec::new(),
        };
        let (canon, r) =
            sat_core::plugin_edit::reconcile(&project.plugins, project.plugins_seen.as_ref(), &disk);
        plugins = PluginNote {
            conflict: r.conflict,
            pending: r.pending,
        };
        project.plugins_seen = Some(disk);
        // the project keeps the plugin canon, so the plugin list works without the build
        project.plugins = canon;
    }

    let staged = mods
        .iter()
        .map(|m| StagedMod {
            name: m.name.clone(),
            route: m.route,
            missing: m.missing,
            error: m.error.clone(),
        })
        .collect();
    Ok((
        Scanned {
            table: builder.finish(),
            plugin_versions: versions,
            mods,
            issues,
            issue_count,
            sync,
            sources,
            plugins,
        },
        Pending {
            run,
            expected: jobs.iter().map(|j| j.name.clone()).collect(),
            staged,
            writer,
        },
    ))
}

/// Conflict totals the seals check (from the caller's conflicts).
#[derive(Debug, Clone, Copy, Default)]
pub struct Totals {
    pub chains: usize,
    pub contexts: usize,
    pub conflict_paths: usize,
}

/// How a run ended.
#[derive(Debug)]
pub struct Published {
    /// Run lifecycle: final status, transitions.
    pub run: RunState,
    /// Seal violations; not empty — nothing was published.
    pub seals: Vec<String>,
    /// Mods whose facts are degraded (unreadable subfolder, …).
    pub degraded: Vec<String>,
    /// The project file written (facts and the project as the run left it).
    pub saved: Option<Saved>,
}

impl Pending<'_> {
    // @sat:home.run.publish
    /// Seals the run and, when they pass and `save` is asked, writes the project (`Published::saved`). A failed seal
    /// publishes nothing: the half-written file goes away, the project on disk stays as it was.
    pub fn publish(mut self, project: &Project, totals: Totals, save: bool) -> Result<Published, String> {
        self.run
            .go(RunStatus::Staging, "projection built")
            .map_err(|e| e.to_string())?;
        let degraded: Vec<String> = self
            .staged
            .iter()
            .filter_map(|m| m.error.as_ref().map(|e| format!("{}: {e}", m.name)))
            .collect();
        let seals = seal(&RunSummary {
            expected: self.expected.into_iter().collect(),
            staged: self.staged,
            chains: totals.chains,
            contexts: totals.contexts,
            conflict_paths: totals.conflict_paths,
        });
        if seals.is_empty() {
            for (to, why) in [
                (RunStatus::Committing, "seals passed"),
                (RunStatus::Verifying, "project written"),
                (RunStatus::Promoting, "verified"),
                (RunStatus::Completed, "published"),
            ] {
                self.run.go(to, why).map_err(|e| e.to_string())?;
            }
        } else {
            self.run
                .go(RunStatus::Failed, "seals failed")
                .map_err(|e| e.to_string())?;
        }
        let saved = match self.writer {
            Some(w) if save && seals.is_empty() => Some(
                w.finish(project, sat_stll::file::keep_entities(project))
                    .map_err(|e| e.to_string())?,
            ),
            _ => None,
        };
        Ok(Published {
            run: self.run,
            seals,
            degraded,
            saved,
        })
    }
}
