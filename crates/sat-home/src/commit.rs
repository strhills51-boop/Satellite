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
    Ok(Committed { report, notes })
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
