//! `sat-cli sync` and `sat-cli commit`: decide sync records, then commit them to the build. @sat:cli.commit
//!
//! Owns: the command line around the commit pipeline — listing the records of the last analysis, recording a
//! decision in the project, printing the plan, and running it only when asked (`--apply`).
//! Not here: the plan (`@sat:sync.plan`), running it (`@sat:mo2.commit`), finding the records (`analyze`).
//!
//! The pipeline of the owner's philosophy: scan/sync (`analyze`) → process (decide records, edit the canon) →
//! commit (this). Without `--apply` nothing on disk changes; only the project file receives decisions.

use std::path::PathBuf;

use sat_core::project::Project;
use sat_core::sync::plan::{Blocker, DiskOp, ProjectOp, plan_with};
use sat_core::sync::records::{Mismatch, Resolution, Status, SyncRecord};
use sat_mo2::commit::{Faults, Layout, Outcome, PROJECT_BEFORE, Payload, execute, stamp_now};
use sat_mo2::profile;

use crate::{flag_value, has, open_instance, project_io, subfolders};

fn load(args: &[String]) -> Result<(sat_mo2::Instance, Layout, sat_stll::Stll, Project), String> {
    let inst = open_instance(args)?;
    let layout = Layout::of(&inst);
    let (stll, project) = project_io::require(&layout.project_file)?;
    Ok((inst, layout, stll, project))
}

fn describe(r: &SyncRecord) -> String {
    let what = match &r.mismatch {
        Mismatch::FsCreated { fs_name } => format!("new folder        {fs_name}"),
        Mismatch::FsDeleted { project_name } => format!("folder missing    {project_name}"),
        Mismatch::FsRenamed {
            project_name,
            fs_name,
        } => format!("folder renamed    {project_name} → {fs_name}"),
    };
    let status = match r.status {
        Status::Pending => "pending".to_string(),
        Status::Ignored => "ignored".to_string(),
        Status::Confirmed => format!(
            "confirmed: {}",
            r.resolution.map_or("—".into(), |x| serde_json::to_string(&x)
                .unwrap_or_default()
                .replace('"', ""))
        ),
    };
    format!("{what}   [{status}]")
}

fn answers(m: &Mismatch) -> &'static str {
    match m {
        Mismatch::FsCreated { .. } => "adopt_to_project | move_to_deleted",
        Mismatch::FsDeleted { .. } => "create_disk_folder | remove_from_project",
        Mismatch::FsRenamed { .. } => "accept_fs_name | rename_disk_to_project",
    }
}

// @sat:cli.commit.sync
/// `sat-cli sync <instance> [--decide N ANSWER]...`: lists the records; `--decide` records a decision in the project.
/// ANSWER is a resolution (`adopt_to_project`, …), `ignore` or `pending`.
pub fn sync(args: &[String]) -> Result<(), String> {
    let (_, layout, mut stll, mut project) = load(args)?;
    let decisions: Vec<(&str, &str)> = args
        .windows(3)
        .filter(|w| w[0] == "--decide")
        .map(|w| (w[1].as_str(), w[2].as_str()))
        .collect();
    for (n, answer) in &decisions {
        let i: usize = n
            .parse()
            .map_err(|_| format!("--decide {n}: a record number from the list"))?;
        let r = project
            .sync
            .get_mut(i.wrapping_sub(1))
            .ok_or_else(|| format!("--decide {n}: no record {n}"))?;
        match *answer {
            "ignore" => {
                r.status = Status::Ignored;
                r.resolution = None;
            }
            "pending" => {
                r.status = Status::Pending;
                r.resolution = None;
            }
            a => {
                let res: Resolution = serde_json::from_str(&format!("\"{a}\"")).map_err(|_| {
                    format!(
                        "--decide {n} {a}: answers for this record: {}",
                        answers(&r.mismatch)
                    )
                })?;
                if !res.fits(&r.mismatch) {
                    return Err(format!(
                        "--decide {n} {a}: answers for this record: {}",
                        answers(&r.mismatch)
                    ));
                }
                r.status = Status::Confirmed;
                r.resolution = Some(res);
            }
        }
    }
    if !decisions.is_empty() {
        project_io::save(&mut stll, &project)?;
        eprintln!("decisions saved to {}", layout.project_file.display());
    }
    if project.sync.is_empty() {
        println!("no sync records: the project and the mods folder agree (as of the last analysis)");
    }
    for (i, r) in project.sync.iter().enumerate() {
        println!("{:>3}. {}", i + 1, describe(r));
        if r.status == Status::Pending {
            println!("       answers: {} | ignore", answers(&r.mismatch));
        }
    }
    Ok(())
}

// @sat:cli.commit.run
/// `sat-cli commit <instance> [--apply]`: prints the plan; with `--apply` runs it (backup, journal, rollback).
pub fn commit(args: &[String]) -> Result<(), String> {
    let (inst, layout, mut stll, project) = load(args)?;
    let disk = subfolders(&inst.mods_dir);
    let planned = project.planned();
    let p = plan_with(&project.canon, &project.sync, &planned, &disk, &|s| {
        crate::intent_cmd::source_ok(&inst, s)
    });
    println!("commit plan for {}:", inst.profile_dir().display());
    for op in &p.project_ops {
        match op {
            ProjectOp::Adopt { name, .. } => println!("  project: add {name} (at the end, enabled)"),
            ProjectOp::Remove { name } => println!("  project: remove {name}"),
            ProjectOp::Rename { from, to } => println!("  project: rename {from} → {to}"),
        }
    }
    for op in &p.disk_ops {
        match op {
            DiskOp::ToDeleted { name } => println!("  disk: move {name} → satellite/deleted/<stamp>/"),
            DiskOp::Rename { from, to } => println!("  disk: rename folder {from} → {to}"),
            DiskOp::Create { name, .. } => println!("  disk: create empty folder {name}"),
            DiskOp::Install { name, source } => {
                let sat_core::intent::Source::Downloads { file, .. } = source;
                println!("  disk: install {name} from downloads/{file}")
            }
        }
    }
    println!(
        "  modlist.txt: written from the project canon ({} entries); {} record(s) settled, {} ignored",
        p.canon_after.entries.len(),
        p.settles.len(),
        p.ignored
    );
    if p.blocked() {
        println!("blocked:");
        for b in &p.blockers {
            println!("  {}", blocker(b, &project.sync));
        }
        return Err("the plan is blocked; nothing was changed".into());
    }
    if !has(args, "--apply") {
        println!("dry run: nothing was changed (add --apply to commit)");
        return Ok(());
    }
    let style = std::fs::read(layout.profile_dir.join("modlist.txt"))
        .map(|d| profile::parse_modlist(&d).1)
        .unwrap_or_default();
    let mut after = project.clone();
    after.apply_plan(&p);
    // the new project file is prepared beside the old one; the commit swaps them inside its transaction
    let prepared = layout.project_file.with_extension("stll-next");
    stll.begin()
        .and_then(|w| w.finish_to(&after, sat_stll::file::keep_entities(&after), &prepared))
        .map_err(|e| e.to_string())?;
    drop(stll);
    let payload = Payload {
        plan: &p,
        modlist: profile::serialize_modlist(&p.canon_after, &style),
        project_after: prepared.clone(),
        plugin_files: None,
    };
    let stamp = flag_value(args, "--stamp").map_or_else(stamp_now, str::to_string);
    let report = execute(&layout, &stamp, &payload, Faults::default());
    println!("version: {}", report.version_dir.display());
    if prepared.exists() {
        let _ = std::fs::remove_file(&prepared);
    }
    match report.outcome {
        Outcome::Committed => {
            // the version keeps the user's truth; the facts are rebuildable and stay in the live project only
            let before = report.version_dir.join(PROJECT_BEFORE);
            if before.exists()
                && let Err(e) = sat_stll::strip_facts(&before)
            {
                eprintln!("{}: facts not stripped: {e}", before.display());
            }
            println!("committed: {} step(s)", report.steps.len());
            Ok(())
        }
        Outcome::RolledBack { error } => Err(format!("failed and rolled back: {error}")),
        Outcome::RollbackIncomplete { error, left } => Err(format!(
            "failed: {error}; NOT undone: {} — see {}",
            left.join("; "),
            PathBuf::from(&report.version_dir)
                .join("commit.journal.json")
                .display()
        )),
        Outcome::Refused { error } => Err(format!("refused: {error}")),
    }
}

fn blocker(b: &Blocker, records: &[SyncRecord]) -> String {
    let rec = |fp: &str| {
        records
            .iter()
            .position(|r| r.fingerprint == fp)
            .map_or(fp.to_string(), |i| {
                format!("record {} ({})", i + 1, describe(&records[i]))
            })
    };
    match b {
        Blocker::Pending { fingerprint } => format!(
            "{} waits for a decision (sat-cli sync --decide)",
            rec(fingerprint)
        ),
        Blocker::NoResolution { fingerprint } => {
            format!("{} is confirmed without an answer", rec(fingerprint))
        }
        Blocker::WrongResolution { fingerprint, .. } => {
            format!("{}: the answer does not fit", rec(fingerprint))
        }
        Blocker::Stale { fingerprint } => format!(
            "{}: the disk changed since the analysis — run analyze",
            rec(fingerprint)
        ),
        Blocker::MissingFolder { name } => format!("folder {name} is not there"),
        Blocker::TargetExists { name } => format!("folder {name} already exists"),
        Blocker::TargetTwice { name } => format!("two operations produce {name}"),
        Blocker::RenameCycle { names } => format!("renames wait on each other: {}", names.join(", ")),
        Blocker::CanonMismatch { name } => format!("{name}: the project list does not fit the change"),
        Blocker::SeparatorNotEmpty { name } => format!("separator {name} still has mods in its block"),
        Blocker::SourceUnavailable { name, why } => format!("planned mod {name}: {why}"),
    }
}
