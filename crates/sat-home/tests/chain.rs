//! The chain end to end on a synthetic build. @sat:home.it
//!
//! Owns: run → a new folder becomes a record → decision → full plan → commit → the next run finds the build and
//! the project in agreement; the sandbox open writes nothing.

use std::fs;
use std::path::Path;

use sat_core::sync::records::{Resolution, Status};
use sat_home::{plan, project, run};
use sat_mo2::Instance;
use sat_mo2::commit::{Layout, Outcome};
use sat_scan::ScanProgress;

fn file(p: &Path, data: &str) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}

/// One run with the project saved; returns the run's sync records and whether the seals passed.
fn run_once(inst: &Instance, save: bool) -> (Vec<sat_core::sync::records::SyncRecord>, bool) {
    let manager = project::manager_list(inst).unwrap();
    let mut o = project::open_for_run(&project::default_path(inst), manager, Default::default()).unwrap();
    let inv = run::inventory(inst, &o.project.canon);
    let jobs = run::jobs(inst, &o.project.canon, &inv.unlisted, true);
    let opts = run::RunOptions::for_build(inst);
    let (scanned, pending) = run::scan(
        inst,
        &mut o.stll,
        &mut o.project,
        &jobs,
        &opts,
        &ScanProgress::default(),
        &mut |_| {},
    )
    .unwrap();
    let published = pending.publish(&o.project, run::Totals::default(), save).unwrap();
    (
        scanned.sync.records,
        published.seals.is_empty() && published.saved.is_some() == save,
    )
}

#[test]
fn run_decide_commit_run() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    file(&base.join("mods/A/textures/a.dds"), "a");
    file(&base.join("mods/B/textures/a.dds"), "b");
    file(&base.join("profiles/Default/modlist.txt"), "+B\n+A\n");
    fs::create_dir_all(base.join("overwrite")).unwrap();
    let inst = Instance::from_dirs(
        base.join("mods"),
        base.join("profiles"),
        base.join("overwrite"),
        None,
    );

    // first run: imported, in agreement
    let (records, ok) = run_once(&inst, true);
    assert!(ok);
    assert!(records.is_empty(), "{records:?}");
    assert!(project::default_path(&inst).exists());

    // a folder appears outside Satellite: the run records it, pending
    file(&base.join("mods/C/meshes/c.nif"), "c");
    let (records, _) = run_once(&inst, true);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, Status::Pending);

    // decided in the project; the plan is blocked until then
    let layout = Layout::of(&inst);
    let (mut stll, mut p) = project::require(&layout.project_file).unwrap();
    assert!(plan::full(&inst, &layout, &p, &[]).plan.blocked());
    p.sync[0].status = Status::Confirmed;
    p.sync[0].resolution = Some(Resolution::AdoptToProject);
    project::save(&mut stll, &p).unwrap();

    let (stll, p) = project::require(&layout.project_file).unwrap();
    let full = plan::full(&inst, &layout, &p, &[]);
    assert!(!full.plan.blocked(), "{:?}", full.plan.blockers);
    let done = sat_home::commit::run(&layout, stll, &p, &full, "t1").unwrap();
    assert_eq!(done.report.outcome, Outcome::Committed);
    let modlist = fs::read_to_string(base.join("profiles/Default/modlist.txt")).unwrap();
    assert!(modlist.contains("+C"), "{modlist}");

    // the next run: nothing to reconcile, the decision settled
    let (records, ok) = run_once(&inst, false);
    assert!(ok);
    assert!(records.is_empty(), "{records:?}");
}

#[test]
fn read_only_open_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    file(&base.join("mods/A/a.esp"), "x");
    file(&base.join("profiles/Default/modlist.txt"), "+A\n");
    let inst = Instance::from_dirs(
        base.join("mods"),
        base.join("profiles"),
        base.join("overwrite"),
        None,
    );
    let manager = project::manager_list(&inst).unwrap();
    let path = project::default_path(&inst);
    let mut o = project::open_for_run(
        &path,
        manager,
        project::OpenOptions {
            reimport: false,
            read_only: true,
        },
    )
    .unwrap();
    assert_eq!(o.origin, project::Origin::Imported);
    let mut opts = run::RunOptions::for_build(&inst);
    opts.save_facts = false;
    opts.scan.cache_dir = None;
    let jobs = run::jobs(&inst, &o.project.canon, &[], true);
    let (_, pending) = run::scan(
        &inst,
        &mut o.stll,
        &mut o.project,
        &jobs,
        &opts,
        &ScanProgress::default(),
        &mut |_| {},
    )
    .unwrap();
    let published = pending
        .publish(&o.project, run::Totals::default(), false)
        .unwrap();
    assert!(published.saved.is_none());
    assert!(!path.exists());
    assert!(!base.join("profiles/Default/satellite").exists());
}
