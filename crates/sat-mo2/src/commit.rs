//! Running a commit plan on an MO2 instance: backup, journal, operations, profile write, rollback. @sat:mo2.commit
//!
//! Owns: the version folder of a commit (backups, intent, journal, manifest), executing the disk operations of a
//! checked plan (`@sat:sync.plan`) one by one with a journal, writing `modlist.txt` and the project atomically, and
//! undoing everything done when a step fails.
//! Not here: what the plan contains and why it may not run (`@sat:sync.plan`), deciding records (`@sat:sync.records`).
//!
//! Order (NG-D003 §7, `11-sync.md` §6 item 5 — the monolith left the build half-changed on any error):
//! 1. `versions/<stamp>/`: `modlist.txt`, `plugins.txt`, `loadorder.txt` copies, `commit.intent.json` (the
//!    plan) — before anything changes;
//! 2. disk operations in plan order, the journal rewritten after each step;
//! 3. `modlist.txt` (with `plugins.txt` and `loadorder.txt` when the commit carries plugin changes) through a
//!    temp file and a rename; then the project: the old file moves into the version folder
//!    (`project.before.stll`) and the new one, prepared beside it before the commit, takes its place — two
//!    renames, whatever the size of the project;
//! 4. on an error in 2–3: every journalled step undone in reverse, the profile files restored from the backup;
//! 5. `commit.rollback.json` — what happened, always written.
//!
//! Folders are never deleted: "delete" moves them to `<instance>/satellite/deleted/<stamp>/`, which sits next to the
//! mods folder in the usual layout, so the move is a rename on one volume. If the mods folder is on another volume
//! the rename fails and the commit rolls back — Satellite does not copy-and-delete mods behind the user's back.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use sat_core::sync::plan::{DiskOp, Plan};

use crate::{Instance, write_atomic};

/// Where a commit reads and writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub mods_dir: PathBuf,
    pub profile_dir: PathBuf,
    /// `<profile>/satellite/versions`.
    pub versions_dir: PathBuf,
    /// `<instance>/satellite/deleted`.
    pub deleted_dir: PathBuf,
    /// The project file of this profile.
    pub project_file: PathBuf,
    /// Archives for installs (MO2 `download_directory`).
    pub downloads_dir: PathBuf,
}

impl Layout {
    pub fn of(inst: &Instance) -> Self {
        let sat = inst.profile_dir().join("satellite");
        Self {
            mods_dir: inst.mods_dir.clone(),
            profile_dir: inst.profile_dir(),
            versions_dir: sat.join("versions"),
            deleted_dir: inst.base_dir.join("satellite").join("deleted"),
            project_file: sat.join("project.stll"),
            downloads_dir: inst.downloads_dir.clone(),
        }
    }
}

/// One executed step, as journalled (enough to undo it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    Moved {
        from: PathBuf,
        to: PathBuf,
    },
    Created {
        path: PathBuf,
    },
    /// A mod folder made from an archive by this commit.
    Installed {
        path: PathBuf,
    },
    /// A profile or project file replaced; `backup` is its copy in the version folder (`None`: it did not exist).
    Wrote {
        path: PathBuf,
        backup: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Committed,
    /// A step failed; everything done was undone.
    RolledBack {
        error: String,
    },
    /// A step failed and undoing failed too: `left` lists what could not be undone (the journal has the rest).
    RollbackIncomplete {
        error: String,
        left: Vec<String>,
    },
    /// Nothing was touched: the plan is blocked or the version folder could not be prepared.
    Refused {
        error: String,
    },
}

/// What a commit did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub stamp: String,
    pub version_dir: PathBuf,
    pub outcome: Outcome,
    pub steps: Vec<Step>,
}

/// What to write at the end.
#[derive(Debug)]
pub struct Payload<'a> {
    pub plan: &'a Plan,
    /// `modlist.txt` of the new canon (`profile::serialize_modlist` of `plan.canon_after`).
    pub modlist: Vec<u8>,
    /// The project as it will be (records settled, canon after): a file prepared in the folder of
    /// `Layout::project_file` (so the swap is a rename). After a rollback it is back at this path.
    pub project_after: PathBuf,
    /// `plugins.txt` and `loadorder.txt` when the commit carries plugin changes (`@sat:plugins.edit.files`,
    /// `profile::serialize_plugins`); `None` leaves both files as they are.
    pub plugin_files: Option<(Vec<u8>, Vec<u8>)>,
}

/// Test hook: fail before step `n` (0-based) of the disk operations / file writes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Faults {
    pub fail_before_step: Option<usize>,
}

fn io(path: &Path, e: std::io::Error) -> String {
    format!("{}: {e}", path.display())
}

/// A name in `dir` not taken yet: `name`, `name__2`, `name__3`…
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let mut p = dir.join(name);
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{name}__{n}"));
        n += 1;
    }
    p
}

/// A version stamp from the clock: `20261101-171503` (UTC).
pub fn stamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

// @sat:mo2.commit.execute
/// Runs a checked plan. Never panics on I/O; the report says what happened.
pub fn execute(layout: &Layout, stamp: &str, payload: &Payload<'_>, faults: Faults) -> Report {
    let version_dir = layout.versions_dir.join(stamp);
    let mut report = Report {
        stamp: stamp.to_string(),
        version_dir: version_dir.clone(),
        outcome: Outcome::Committed,
        steps: Vec::new(),
    };
    if payload.plan.blocked() {
        report.outcome = Outcome::Refused {
            error: format!("the plan is blocked: {} blocker(s)", payload.plan.blockers.len()),
        };
        return report;
    }
    // 1. version folder and backups, before anything changes
    if let Err(error) = prepare(layout, &version_dir, payload) {
        report.outcome = Outcome::Refused { error };
        write_manifest(&version_dir, &report);
        return report;
    }
    // 2–3. steps
    if let Err(error) = run(layout, stamp, &version_dir, payload, faults, &mut report.steps) {
        let left = undo(&report.steps);
        report.outcome = if left.is_empty() {
            Outcome::RolledBack { error }
        } else {
            Outcome::RollbackIncomplete { error, left }
        };
    }
    write_manifest(&version_dir, &report);
    report
}

fn prepare(layout: &Layout, version_dir: &Path, payload: &Payload<'_>) -> Result<(), String> {
    if version_dir.exists() {
        return Err(format!("{} already exists", version_dir.display()));
    }
    std::fs::create_dir_all(version_dir).map_err(|e| io(version_dir, e))?;
    for f in ["modlist.txt", "plugins.txt", "loadorder.txt"] {
        let src = layout.profile_dir.join(f);
        if src.exists() {
            std::fs::copy(&src, version_dir.join(f)).map_err(|e| io(&src, e))?;
        }
    }
    let w = |name: &str, data: &[u8]| {
        std::fs::write(version_dir.join(name), data).map_err(|e| io(&version_dir.join(name), e))
    };
    if !payload.project_after.is_file() {
        return Err(format!(
            "{}: the prepared project is not there",
            payload.project_after.display()
        ));
    }
    let intent = serde_json::to_vec_pretty(payload.plan).map_err(|e| e.to_string())?;
    w("commit.intent.json", &intent)?;
    Ok(())
}

fn journal(version_dir: &Path, steps: &[Step]) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(steps).map_err(|e| e.to_string())?;
    write_atomic(&version_dir.join("commit.journal.json"), &data).map_err(|e| e.to_string())
}

fn run(
    layout: &Layout,
    stamp: &str,
    version_dir: &Path,
    payload: &Payload<'_>,
    faults: Faults,
    steps: &mut Vec<Step>,
) -> Result<(), String> {
    let mut n = 0;
    let gate = |n: &mut usize| {
        let i = *n;
        *n += 1;
        if faults.fail_before_step == Some(i) {
            return Err(format!("injected fault before step {i}"));
        }
        Ok(())
    };
    let deleted = layout.deleted_dir.join(stamp);
    for op in &payload.plan.disk_ops {
        gate(&mut n)?;
        match op {
            DiskOp::ToDeleted { name } => {
                let from = layout.mods_dir.join(name);
                std::fs::create_dir_all(&deleted).map_err(|e| io(&deleted, e))?;
                let to = free_name(&deleted, name);
                std::fs::rename(&from, &to).map_err(|e| io(&from, e))?;
                steps.push(Step::Moved { from, to });
            }
            DiskOp::Rename { from, to } => {
                let (from, to) = (layout.mods_dir.join(from), layout.mods_dir.join(to));
                // the plan checked the names; the disk may have changed since (another program)
                if to.exists() && !same_folder_other_case(&from, &to) {
                    return Err(format!("{} appeared since the plan was made", to.display()));
                }
                std::fs::rename(&from, &to).map_err(|e| io(&from, e))?;
                steps.push(Step::Moved { from, to });
            }
            DiskOp::Create { name, .. } => {
                let path = layout.mods_dir.join(name);
                std::fs::create_dir(&path).map_err(|e| io(&path, e))?;
                steps.push(Step::Created { path });
            }
            // @sat:mo2.commit.install
            DiskOp::Install { name, source, fomod } => {
                let sat_core::intent::Source::Downloads { file, .. } = source;
                let path = layout.mods_dir.join(name);
                crate::install::install(&layout.downloads_dir.join(file), &path, fomod.as_deref())?;
                steps.push(Step::Installed { path });
            }
        }
        journal(version_dir, steps)?;
    }
    let in_profile = |name: &str| {
        let path = layout.profile_dir.join(name);
        let backup = path.exists().then(|| version_dir.join(name));
        (path, backup)
    };
    let mut writes: Vec<(PathBuf, &Vec<u8>, Option<PathBuf>)> = Vec::new();
    let (path, backup) = in_profile("modlist.txt");
    writes.push((path, &payload.modlist, backup));
    if let Some((plugins, loadorder)) = &payload.plugin_files {
        let (path, backup) = in_profile("plugins.txt");
        writes.push((path, plugins, backup));
        let (path, backup) = in_profile("loadorder.txt");
        writes.push((path, loadorder, backup));
    }
    for (path, data, backup) in writes {
        gate(&mut n)?;
        write_atomic(&path, data).map_err(|e| e.to_string())?;
        steps.push(Step::Wrote { path, backup });
        journal(version_dir, steps)?;
    }
    // @sat:mo2.commit.project
    // the project: the old file into the version folder, the prepared one in its place
    gate(&mut n)?;
    let target = layout.project_file.clone();
    if target.exists() {
        let aside = version_dir.join(PROJECT_BEFORE);
        std::fs::rename(&target, &aside).map_err(|e| io(&target, e))?;
        steps.push(Step::Moved {
            from: target.clone(),
            to: aside,
        });
        journal(version_dir, steps)?;
    }
    std::fs::rename(&payload.project_after, &target).map_err(|e| io(&payload.project_after, e))?;
    steps.push(Step::Moved {
        from: payload.project_after.clone(),
        to: target,
    });
    journal(version_dir, steps)?;
    Ok(())
}

/// The project as it was before the commit, in the version folder.
pub const PROJECT_BEFORE: &str = "project.before.stll";

fn same_folder_other_case(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

// @sat:mo2.commit.undo
/// Undoes journalled steps in reverse; returns what could not be undone.
fn undo(steps: &[Step]) -> Vec<String> {
    let mut left = Vec::new();
    for s in steps.iter().rev() {
        let r = match s {
            Step::Moved { from, to } => std::fs::rename(to, from).map_err(|e| io(to, e)),
            // only an empty folder Satellite created itself is removed
            Step::Created { path } => std::fs::remove_dir(path).map_err(|e| io(path, e)),
            // a folder this commit extracted (the archive stays in downloads)
            Step::Installed { path } => std::fs::remove_dir_all(path).map_err(|e| io(path, e)),
            Step::Wrote {
                path,
                backup: Some(b),
            } => std::fs::read(b)
                .map_err(|e| io(b, e))
                .and_then(|data| write_atomic(path, &data).map_err(|e| e.to_string())),
            Step::Wrote { path, backup: None } => std::fs::remove_file(path).map_err(|e| io(path, e)),
        };
        if let Err(e) = r {
            left.push(e);
        }
    }
    left
}

fn write_manifest(version_dir: &Path, report: &Report) {
    if let Ok(data) = serde_json::to_vec_pretty(report) {
        let _ = std::fs::create_dir_all(version_dir);
        let _ = write_atomic(&version_dir.join("commit.rollback.json"), &data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::sync::plan::plan;
    use sat_core::sync::records::{EntityKind, Mismatch, Resolution, Status, SyncRecord};

    struct Build {
        _dir: tempfile::TempDir,
        layout: Layout,
    }

    /// A build: mods `A`, `Junk`, `Renamed`; the canon knows `A`, `Old`, `Lost`.
    fn build() -> Build {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let mods = base.join("mods");
        for m in ["A", "Junk", "Renamed"] {
            std::fs::create_dir_all(mods.join(m)).unwrap();
            std::fs::write(mods.join(m).join("file.txt"), m).unwrap();
        }
        let profile = base.join("profiles").join("Default");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("modlist.txt"), "+Lost\n+Old\n+A\n").unwrap();
        let sat = profile.join("satellite");
        std::fs::create_dir_all(&sat).unwrap();
        std::fs::write(sat.join("project.stll"), "before").unwrap();
        std::fs::write(sat.join("project.next"), "after").unwrap();
        Build {
            layout: Layout {
                mods_dir: mods,
                profile_dir: profile,
                versions_dir: sat.join("versions"),
                deleted_dir: base.join("satellite").join("deleted"),
                project_file: sat.join("project.stll"),
                downloads_dir: base.join("downloads"),
            },
            _dir: dir,
        }
    }

    fn the_plan() -> Plan {
        let canon = Modlist::new(
            ["A", "Old", "Lost"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: true,
                })
                .collect(),
        );
        let rec = |m: Mismatch, r: Resolution| SyncRecord {
            fingerprint: format!("{m:?}"),
            entity: EntityKind::Mod,
            mismatch: m,
            status: Status::Confirmed,
            resolution: Some(r),
        };
        let recs = vec![
            rec(
                Mismatch::FsCreated {
                    fs_name: "Junk".into(),
                },
                Resolution::MoveToDeleted,
            ),
            rec(
                Mismatch::FsRenamed {
                    project_name: "Old".into(),
                    fs_name: "Renamed".into(),
                },
                Resolution::RenameDiskToProject,
            ),
            rec(
                Mismatch::FsDeleted {
                    project_name: "Lost".into(),
                },
                Resolution::CreateDiskFolder,
            ),
        ];
        let p = plan(&canon, &recs, &["A".into(), "Junk".into(), "Renamed".into()]);
        assert!(!p.blocked(), "{:?}", p.blockers);
        p
    }

    fn folders(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn payload<'a>(p: &'a Plan, b: &Build) -> Payload<'a> {
        Payload {
            plan: p,
            modlist: b"+Lost\n+Old\n+A\n# after\n".to_vec(),
            project_after: b.layout.project_file.with_file_name("project.next"),
            plugin_files: None,
        }
    }

    #[test]
    fn a_commit_moves_renames_creates_and_writes_with_a_backup() {
        let b = build();
        let p = the_plan();
        let r = execute(&b.layout, "20261101-120000", &payload(&p, &b), Faults::default());
        assert_eq!(r.outcome, Outcome::Committed);
        assert_eq!(folders(&b.layout.mods_dir), ["A", "Lost", "Old"]);
        // the moved folder is whole in deleted/<stamp>, never removed
        let gone = b
            .layout
            .deleted_dir
            .join("20261101-120000")
            .join("Junk")
            .join("file.txt");
        assert_eq!(std::fs::read_to_string(gone).unwrap(), "Junk");
        assert!(
            std::fs::read_to_string(b.layout.profile_dir.join("modlist.txt"))
                .unwrap()
                .ends_with("# after\n")
        );
        assert_eq!(std::fs::read_to_string(&b.layout.project_file).unwrap(), "after");
        let v = &r.version_dir;
        assert_eq!(
            std::fs::read_to_string(v.join("modlist.txt")).unwrap(),
            "+Lost\n+Old\n+A\n"
        );
        for f in [
            PROJECT_BEFORE,
            "commit.intent.json",
            "commit.journal.json",
            "commit.rollback.json",
        ] {
            assert!(v.join(f).exists(), "{f}");
        }
    }

    #[test]
    fn a_failure_half_way_undoes_every_step() {
        // fail at each step in turn: disk ops 0..3, modlist write 3, project write 4
        for at in 0..5 {
            let b = build();
            let p = the_plan();
            let r = execute(
                &b.layout,
                "s",
                &payload(&p, &b),
                Faults {
                    fail_before_step: Some(at),
                },
            );
            assert!(
                matches!(r.outcome, Outcome::RolledBack { .. }),
                "at {at}: {:?}",
                r.outcome
            );
            assert_eq!(folders(&b.layout.mods_dir), ["A", "Junk", "Renamed"], "at {at}");
            assert_eq!(
                std::fs::read_to_string(b.layout.profile_dir.join("modlist.txt")).unwrap(),
                "+Lost\n+Old\n+A\n",
                "at {at}"
            );
            assert_eq!(
                std::fs::read_to_string(&b.layout.project_file).unwrap(),
                "before",
                "at {at}"
            );
            assert_eq!(
                std::fs::read_to_string(b.layout.project_file.with_file_name("project.next")).unwrap(),
                "after",
                "at {at}: the prepared project is back where it was"
            );
            assert!(r.version_dir.join("commit.rollback.json").exists());
        }
    }

    #[test]
    fn a_blocked_plan_touches_nothing() {
        let b = build();
        let mut p = the_plan();
        p.blockers.push(sat_core::sync::plan::Blocker::Pending {
            fingerprint: "x".into(),
        });
        let r = execute(&b.layout, "s", &payload(&p, &b), Faults::default());
        assert!(matches!(r.outcome, Outcome::Refused { .. }));
        assert!(!r.version_dir.exists());
        assert_eq!(folders(&b.layout.mods_dir), ["A", "Junk", "Renamed"]);
    }

    #[test]
    fn a_stamp_looks_like_a_date() {
        let s = stamp_now();
        assert_eq!(s.len(), 15);
        assert!(s.starts_with("20") && s.as_bytes()[8] == b'-');
    }

    #[test]
    fn plugin_files_are_written_and_restored_with_the_rest() {
        let b = build();
        std::fs::write(b.layout.profile_dir.join("plugins.txt"), "*Old.esp\n").unwrap();
        let p = the_plan();
        let mut pl = payload(&p, &b);
        pl.plugin_files = Some((b"*New.esp\n".to_vec(), b"New.esp\n".to_vec()));
        // fail at the project write (after modlist, plugins, loadorder): all three come back
        let r = execute(
            &b.layout,
            "s",
            &pl,
            Faults {
                fail_before_step: Some(6),
            },
        );
        assert!(matches!(r.outcome, Outcome::RolledBack { .. }), "{:?}", r.outcome);
        assert_eq!(
            std::fs::read_to_string(b.layout.profile_dir.join("plugins.txt")).unwrap(),
            "*Old.esp\n"
        );
        assert!(
            !b.layout.profile_dir.join("loadorder.txt").exists(),
            "a file that did not exist is removed again"
        );
        let b = build();
        let pl = Payload {
            project_after: b.layout.project_file.with_file_name("project.next"),
            ..pl
        };
        let r = execute(&b.layout, "t", &pl, Faults::default());
        assert_eq!(r.outcome, Outcome::Committed);
        assert_eq!(
            std::fs::read_to_string(b.layout.profile_dir.join("loadorder.txt")).unwrap(),
            "New.esp\n"
        );
    }

    #[test]
    fn an_install_is_undone_with_the_rest() {
        use std::io::Write;
        let b = build();
        let dl = b.layout.downloads_dir.clone();
        std::fs::create_dir_all(&dl).unwrap();
        let mut w = zip::ZipWriter::new(std::fs::File::create(dl.join("New.zip")).unwrap());
        w.start_file("textures/a.dds", zip::write::SimpleFileOptions::default())
            .unwrap();
        w.write_all(b"a").unwrap();
        w.finish().unwrap();
        let mut p = the_plan();
        p.disk_ops.push(DiskOp::Install {
            name: "New".into(),
            source: sat_core::intent::Source::Downloads {
                file: "New.zip".into(),
                size: None,
            },
            fomod: None,
        });
        // 4 disk ops (0..3), then modlist (4): fail there — the installed folder goes away
        let r = execute(
            &b.layout,
            "s",
            &payload(&p, &b),
            Faults {
                fail_before_step: Some(4),
            },
        );
        assert!(matches!(r.outcome, Outcome::RolledBack { .. }), "{:?}", r.outcome);
        assert_eq!(folders(&b.layout.mods_dir), ["A", "Junk", "Renamed"]);
        let r = execute(&b.layout, "t", &payload(&p, &b), Faults::default());
        assert_eq!(r.outcome, Outcome::Committed);
        assert!(b.layout.mods_dir.join("New/textures/a.dds").exists());
    }
}
