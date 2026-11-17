//! Running a full plan on the build (the commit transaction plus the home's steps around it). @sat:home.commit
//!
//! Owns: turning a [`FullPlan`](crate::plan::FullPlan) into the commit's payload (`modlist.txt` in the profile's
//! style, the plugin files, the prepared project) and the steps around `@sat:mo2.commit` that are the home's: the
//! prepared file removed afterwards, the facts stripped from the version's copy of the project.
//! Not here: the commit transaction itself — backup, journal, rollback (`@sat:mo2.commit`).

use sat_core::project::Project;
use sat_mo2::commit::{Faults, Layout, Outcome, PROJECT_BEFORE, Payload, Report, execute};
use sat_mo2::profile;
use sat_stll::Stll;

use crate::plan::{FullPlan, project_after};

/// What a commit did, and what the home could not tidy after it.
#[derive(Debug)]
pub struct Committed {
    pub report: Report,
    /// Problems after a successful commit that do not undo it (the version's project kept its facts…).
    pub notes: Vec<String>,
    /// The processors run after the commit (`@sat:home.commit.processors`).
    pub processed: Vec<sat_mo2::processors::Done>,
}

// @sat:home.commit.run
/// Runs `f` (not blocked — the caller checked) on the build. The project file is replaced inside the commit's
/// transaction; `stll` is consumed because the file under it changes.
pub fn run(
    layout: &Layout,
    stll: Stll,
    project: &Project,
    f: &FullPlan,
    stamp: &str,
) -> Result<Committed, String> {
    let style = std::fs::read(layout.profile_dir.join("modlist.txt"))
        .map(|d| profile::parse_modlist(&d).1)
        .unwrap_or_default();
    let after = project_after(project, f);
    // the new project file is prepared beside the old one; the commit swaps them inside its transaction
    let prepared = layout.project_file.with_extension("stll-next");
    let mut stll = stll;
    stll.begin()
        .and_then(|w| w.finish_to(&after, sat_stll::file::keep_entities(&after), &prepared))
        .map_err(|e| e.to_string())?;
    drop(stll);
    let payload = Payload {
        plan: &f.plan,
        modlist: profile::serialize_modlist(&f.plan.canon_after, &style),
        project_after: prepared.clone(),
        plugin_files: f
            .plugins
            .as_ref()
            .map(|w| (w.plugins.clone(), w.loadorder.clone())),
    };
    let report = execute(layout, stamp, &payload, Faults::default());
    if prepared.exists() {
        let _ = std::fs::remove_file(&prepared);
    }
    let mut notes = Vec::new();
    if report.outcome == Outcome::Committed {
        // the version keeps the user's truth; the facts are rebuildable and stay in the live project only
        let before = report.version_dir.join(PROJECT_BEFORE);
        if before.exists()
            && let Err(e) = sat_stll::strip_facts(&before)
        {
            notes.push(format!("{}: facts not stripped: {e}", before.display()));
        }
    }
    let processed = if report.outcome == Outcome::Committed && !f.processors.is_empty() {
        processors(layout, f, stamp, &mut notes)
    } else {
        Vec::new()
    };
    Ok(Committed {
        report,
        notes,
        processed,
    })
}

// @sat:home.commit.processors
/// The second phase of a commit (ADR 0021): with the profile written, each dirty processor's tool runs through MO2
/// and its output is collected; a promoted run's fingerprint goes into the project file. MO2 open or not found —
/// nothing runs (said in the notes), the processors stay dirty for the next commit.
fn processors(
    layout: &Layout,
    f: &FullPlan,
    stamp: &str,
    notes: &mut Vec<String>,
) -> Vec<sat_mo2::processors::Done> {
    let Some((exe, profile)) = f.mo2.clone() else {
        notes.push("processors: ModOrganizer.exe of this build not found — none ran".into());
        return Vec::new();
    };
    if sat_mo2::processors::mo2_running() {
        notes.push("processors: MO2 is open — close it and commit again to run them".into());
        return Vec::new();
    }
    let place = sat_mo2::processors::Place {
        mods_dir: layout.mods_dir.clone(),
        overwrite_dir: f.overwrite_dir.clone(),
        deleted_dir: layout.deleted_dir.join(stamp),
    };
    let launch = sat_mo2::processors::launcher(exe, profile);
    run_processors(layout, f, &place, &launch, notes)
}

/// Runs the plan's processors with `launch` and records the promoted ones in the project file.
pub fn run_processors(
    layout: &Layout,
    f: &FullPlan,
    place: &sat_mo2::processors::Place,
    launch: &dyn Fn(&sat_mo2::processors::Run) -> Result<(), String>,
    notes: &mut Vec<String>,
) -> Vec<sat_mo2::processors::Done> {
    let done: Vec<_> = f
        .processors
        .iter()
        .map(|(_, r)| sat_mo2::processors::run(place, r, launch))
        .collect();
    for d in &done {
        if let Some(e) = &d.error {
            notes.push(format!("processor {}: {e}", d.id));
        }
    }
    if done.iter().any(|d| d.promoted) {
        let saved = crate::project::open(&layout.project_file).and_then(|o| {
            let (mut stll, mut p) = o.ok_or("the project file is gone")?;
            for (d, (_, r)) in done.iter().zip(&f.processors) {
                if let Some(x) = p
                    .processors
                    .iter_mut()
                    .find(|x| x.id == r.id)
                    .filter(|_| d.promoted)
                {
                    x.built = Some(r.fingerprint.clone());
                }
            }
            crate::project::save(&mut stll, &p)
        });
        if let Err(e) = saved {
            notes.push(format!("processors ran, but the project did not record it: {e}"));
        }
    }
    done
}

// @sat:home.commit.fresh
/// The freshness gate before a commit writes (ADR 0019 §4, monolith pre-commit): a surface run over the mods
/// folder now must find the same mismatches the project's records describe — a folder that appeared, went or was
/// renamed since the last run makes the plan stale. Returns what differs (record fingerprints), empty when fresh.
pub fn stale(inst: &sat_mo2::Instance, project: &Project) -> Vec<String> {
    let mut now = project.clone();
    let seen = crate::run::surface(inst, &mut now);
    let had: std::collections::BTreeSet<&str> = project.sync.iter().map(|r| r.fingerprint.as_str()).collect();
    let has: std::collections::BTreeSet<&str> = seen.records.iter().map(|r| r.fingerprint.as_str()).collect();
    had.symmetric_difference(&has).map(|s| s.to_string()).collect()
}
