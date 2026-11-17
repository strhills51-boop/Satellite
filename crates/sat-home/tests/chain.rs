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
    drop(stll);

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

#[test]
fn surface_finds_what_the_full_run_finds() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    file(&base.join("mods/A/textures/a.dds"), "a");
    file(&base.join("mods/B/textures/b.dds"), "bb");
    file(&base.join("mods/Extra_separator/meta.ini"), "");
    file(&base.join("profiles/Default/modlist.txt"), "+B\n+A\n");
    fs::create_dir_all(base.join("overwrite")).unwrap();
    let inst = Instance::from_dirs(
        base.join("mods"),
        base.join("profiles"),
        base.join("overwrite"),
        None,
    );
    let (records, _) = run_once(&inst, true);
    assert!(
        records.iter().all(|r| r.fingerprint.contains("separator")),
        "{records:?}"
    );
    // B renamed outside Satellite, a new folder, A gone
    fs::rename(base.join("mods/B"), base.join("mods/B2")).unwrap();
    file(&base.join("mods/C/c.esp"), "c");
    fs::remove_dir_all(base.join("mods/A")).unwrap();
    let (_, mut p) = project::require(&project::default_path(&inst)).unwrap();
    let mut q = p.clone();
    let surface = run::surface(&inst, &mut p).records;
    let manager = project::manager_list(&inst).unwrap();
    let mut o = project::open_for_run(&project::default_path(&inst), manager, Default::default()).unwrap();
    let inv = run::inventory(&inst, &o.project.canon);
    let jobs = run::jobs(&inst, &o.project.canon, &inv.unlisted, true);
    let mut opts = run::RunOptions::for_build(&inst);
    opts.save_facts = false;
    let (full, _) = run::scan(
        &inst,
        &mut o.stll,
        &mut o.project,
        &jobs,
        &opts,
        &ScanProgress::default(),
        &mut |_| {},
    )
    .unwrap();
    assert_eq!(surface, full.sync.records);
    let kinds: Vec<&str> = surface
        .iter()
        .map(|r| r.fingerprint.split("::").next().unwrap())
        .collect();
    assert!(kinds.contains(&"fs_renamed"), "{surface:?}");
    assert!(
        kinds.contains(&"fs_created") && kinds.contains(&"fs_deleted"),
        "{surface:?}"
    );
    // the surface run does not touch the facts: only records and signatures
    let r2 = run::surface(&inst, &mut q);
    assert_eq!(r2.records, surface);
}

#[test]
fn save_refuses_a_file_changed_outside() {
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
    let path = project::default_path(&inst);
    let p = sat_core::project::Project::import(project::manager_list(&inst).unwrap());
    // no file yet: saved, the stamp is the new file's
    let s1 = project::save_checked(&path, &p, None, false).unwrap();
    assert_eq!(project::stamp_of(&path), Some(s1));
    let mut q = p.clone();
    q.pinned_files.push("x".into());
    let s2 = project::save_checked(&path, &q, Some(s1), false).unwrap();
    // another program writes it: refused, unless forced
    std::thread::sleep(std::time::Duration::from_millis(20));
    let (mut stll, mut r) = project::require(&path).unwrap();
    r.pinned_files.push("y".into());
    project::save(&mut stll, &r).unwrap();
    // that program is done with the file (Windows does not replace a file another handle keeps open)
    drop(stll);
    assert_eq!(
        project::save_checked(&path, &q, Some(s2), false),
        Err(project::SaveError::ChangedOutside)
    );
    project::save_checked(&path, &q, Some(s2), true).unwrap();
    assert_eq!(project::require(&path).unwrap().1.pinned_files, ["x"]);
}

#[test]
fn a_folder_changed_after_the_run_makes_the_commit_stale() {
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
    run_once(&inst, true);
    let (_, p) = project::require(&project::default_path(&inst)).unwrap();
    assert!(sat_home::commit::stale(&inst, &p).is_empty());
    file(&base.join("mods/B/b.esp"), "y");
    assert_eq!(sat_home::commit::stale(&inst, &p), ["fs_created::mod::b"]);
}

#[test]
fn reimport_takes_the_canon_and_keeps_the_rest() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    file(&base.join("mods/A/a.esp"), "x");
    file(&base.join("mods/B/b.esp"), "y");
    file(&base.join("profiles/Default/modlist.txt"), "+B\n+A\n");
    let inst = Instance::from_dirs(
        base.join("mods"),
        base.join("profiles"),
        base.join("overwrite"),
        None,
    );
    let path = project::default_path(&inst);
    let mut p = sat_core::project::Project::import(project::manager_list(&inst).unwrap());
    p.pinned_files.push("a.esp".into());
    p.patches.push(sat_core::patch::FilePatch {
        source: p.entities.id_of("A").unwrap().0.clone(),
        path: "x.ini".into(),
        base: None,
        ops: Vec::new(),
    });
    p.plugins_seen = Some(Default::default());
    let id_a = p.entities.id_of("A").unwrap().clone();
    project::save_checked(&path, &p, None, false).unwrap();
    // the manager's order changed and a mod was added there
    file(&base.join("mods/C/c.esp"), "z");
    file(&base.join("profiles/Default/modlist.txt"), "+C\n+A\n+B\n");
    let o = project::open_for_run(
        &path,
        project::manager_list(&inst).unwrap(),
        project::OpenOptions {
            reimport: true,
            read_only: false,
        },
    )
    .unwrap();
    assert_eq!(o.origin, project::Origin::Reimported);
    let names: Vec<&str> = o.project.canon.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["B", "A", "C"]);
    assert_eq!(o.project.entities.id_of("A"), Some(&id_a), "ids kept by name");
    assert!(o.project.entities.id_of("C").is_some());
    assert_eq!(o.project.pinned_files, ["a.esp"]);
    assert_eq!(o.project.patches.len(), 1);
    assert!(o.project.plugins_seen.is_some());
}

#[test]
fn a_dirty_processor_runs_after_the_commit_and_its_output_is_collected() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    file(&base.join("mods/Anims/pandora_engine/patch.txt"), "p");
    file(&base.join("mods/Armor/textures/a.dds"), "a");
    file(&base.join("profiles/Default/modlist.txt"), "+Armor\n+Anims\n");
    fs::create_dir_all(base.join("overwrite")).unwrap();
    file(&base.join("tools/Pandora Behaviour Engine+.exe"), "exe");
    let tool = base
        .join("tools/Pandora Behaviour Engine+.exe")
        .display()
        .to_string()
        .replace('\\', "/");
    file(
        &base.join("ModOrganizer.ini"),
        &format!("[customExecutables]\nsize=1\n1\\title=Pandora\n1\\binary={tool}\n"),
    );
    let mut inst = Instance::from_dirs(
        base.join("mods"),
        base.join("profiles"),
        base.join("overwrite"),
        None,
    );
    inst.ini_path = base.join("ModOrganizer.ini");
    assert!(run_once(&inst, true).1);

    // the tool is found in MO2's list; its processor is taken into the project and turned on
    let layout = Layout::of(&inst);
    let (mut stll, mut p) = project::require(&layout.project_file).unwrap();
    let tools: Vec<(String, String)> = sat_mo2::executables::read(&inst.ini_path)
        .iter()
        .map(|t| (t.title.clone(), t.file_name()))
        .collect();
    p.processors = sat_core::processor::detect(&tools, &[]);
    assert_eq!(p.processors.len(), 1);
    p.processors[0].enabled = true;
    project::save(&mut stll, &p).unwrap();
    drop(stll);

    let (stll, p) = project::require(&layout.project_file).unwrap();
    let full = plan::full(&inst, &layout, &p, &[]);
    assert_eq!(full.processors.len(), 1, "never built: dirty");
    assert!(
        full.plan
            .canon_after
            .entries
            .iter()
            .any(|e| e.name == "Pandora Output"),
        "the output is listed"
    );
    let done = sat_home::commit::run(&layout, stll, &p, &full, "t1").unwrap();
    assert_eq!(done.report.outcome, Outcome::Committed);
    assert!(done.processed.is_empty(), "no ModOrganizer.exe here: nothing ran");
    assert!(
        base.join("mods/Pandora Output").is_dir(),
        "its folder is made for MO2 to list"
    );

    // the run itself, with a stand-in for MO2 and the tool: it writes to Overwrite
    let ow = base.join("overwrite");
    let fake = move |_: &sat_mo2::processors::Run| {
        file(
            &ow.join("meshes/actors/character/behaviors/0_master.hkx"),
            "built",
        );
        Ok(())
    };
    let place = sat_mo2::processors::Place {
        mods_dir: base.join("mods"),
        overwrite_dir: base.join("overwrite"),
        deleted_dir: base.join("deleted/t1"),
    };
    let mut notes = Vec::new();
    let done = sat_home::commit::run_processors(&layout, &full, &place, &fake, &mut notes);
    assert!(done[0].promoted, "{notes:?}");
    assert!(
        base.join("mods/Pandora Output/meshes/actors/character/behaviors/0_master.hkx")
            .exists()
    );
    // recorded: the next plan finds it clean
    let (_, p) = project::require(&layout.project_file).unwrap();
    assert!(p.processors[0].built.is_some());
    assert!(
        plan::full(&inst, &layout, &p, &[]).processors.is_empty(),
        "clean after the run"
    );
}
