//! Sync records and the commit plan through the application API (NG-D003 §5, §7). @sat:app.sync
//!
//! Owns: the records of the last analysis held by the app, deciding them by command, and the plan read as a query
//! (pure: a plan never runs from here — running it is the shell's step with `@sat:mo2.commit`, behind a backup).
//! Not here: the reconcile (`@sat:sync.records`), the plan (`@sat:sync.plan`), the disk (`@sat:mo2.commit`).

use serde::{Deserialize, Serialize};

use sat_core::sync::plan::{Plan, plan};
use sat_core::sync::records::{Mismatch, Resolution, Status, SyncRecord};

use crate::{App, AppError};

/// A decision on one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Pending,
    Ignore,
    Confirm { resolution: Resolution },
}

/// One record as the window shows it, with the answers it accepts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordView {
    #[serde(flatten)]
    pub record: SyncRecord,
    pub answers: Vec<Resolution>,
}

/// What the app knows of the disk side: the records and the folder names they were found against.
#[derive(Debug, Clone, Default)]
pub(crate) struct SyncState {
    pub records: Vec<SyncRecord>,
    pub disk: Vec<String>,
}

pub(crate) fn answers(m: &Mismatch) -> Vec<Resolution> {
    use Resolution as R;
    [
        R::AdoptToProject,
        R::MoveToDeleted,
        R::CreateDiskFolder,
        R::RemoveFromProject,
        R::AcceptFsName,
        R::RenameDiskToProject,
    ]
    .into_iter()
    .filter(|r| r.fits(m))
    .collect()
}

impl App {
    /// The shell hands over the records of an analysis and the folder names it saw.
    pub fn set_sync(&mut self, records: Vec<SyncRecord>, disk: Vec<String>) {
        self.sync = SyncState { records, disk };
    }

    /// Records with their decisions (what the shell saves into the project).
    pub fn sync_records(&self) -> &[SyncRecord] {
        &self.sync.records
    }

    pub(crate) fn record_views(&self) -> Vec<RecordView> {
        self.sync
            .records
            .iter()
            .map(|r| RecordView {
                answers: answers(&r.mismatch),
                record: r.clone(),
            })
            .collect()
    }

    // @sat:app.sync.decide
    /// Records a decision; `false` when nothing changed.
    pub(crate) fn decide(&mut self, fingerprint: &str, d: Decision) -> Result<bool, AppError> {
        let r = self
            .sync
            .records
            .iter_mut()
            .find(|r| r.fingerprint == fingerprint)
            .ok_or_else(|| AppError::Record(fingerprint.to_string()))?;
        let (status, resolution) = match d {
            Decision::Pending => (Status::Pending, None),
            Decision::Ignore => (Status::Ignored, None),
            Decision::Confirm { resolution } if resolution.fits(&r.mismatch) => {
                (Status::Confirmed, Some(resolution))
            }
            Decision::Confirm { .. } => {
                return Err(AppError::Record(format!(
                    "{fingerprint}: the answer does not fit"
                )));
            }
        };
        let changed = r.status != status || r.resolution != resolution;
        r.status = status;
        r.resolution = resolution;
        Ok(changed)
    }

    /// The shell gives the full plan of its build (`@sat:home.plan`).
    pub fn set_plan_source(&mut self, s: std::sync::Arc<dyn PlanSource>) {
        self.plan_source = Some(s);
    }

    // @sat:app.sync.full
    /// The commit plan: with a build, the full plan the commit would run (the shell's, the same as `sat-cli
    /// commit`); without one, the plan over the current canon and the decided records.
    pub(crate) fn commit_plan(&mut self) -> (Plan, Option<PlanExtras>) {
        if let Some(src) = self.plan_source.clone()
            && let Some(p) = self.project()
        {
            let (plan, extras) = src.full_plan(&p, &[]);
            return (plan, Some(extras));
        }
        let canon = self.session.book().to_modlist();
        (plan(&canon, &self.sync.records, &self.sync.disk), None)
    }
}

/// The full commit plan of a build, given by the shell (it knows the disk; the app does not, ADR 0004).
pub trait PlanSource: Send + Sync + std::fmt::Debug {
    /// The plan of `project` against the build now; `confirmed` names steps the user confirmed.
    fn full_plan(&self, project: &sat_core::project::Project, confirmed: &[String]) -> (Plan, PlanExtras);
}

/// What a commit writes besides the plan's operations.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanExtras {
    /// `plugins.txt` / `loadorder.txt`: written from the project (its own edits), kept, plus plugins of packed
    /// outputs enabled at the end.
    pub plugins_written: bool,
    pub plugins_packed: Vec<String>,
    /// Factory outputs built and groups sealed.
    pub factories: Vec<String>,
    pub sealed: Vec<String>,
    /// The patches output: written (`Some(true)`), taken away (`Some(false)`), untouched.
    pub patches: Option<bool>,
    /// What the shell said about the plan (a factory whose group is gone, plugin lists changed on both sides).
    pub notes: Vec<String>,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::order::OrderBook;
    use sat_core::resource::Registry;
    use sat_core::session::Session;
    use sat_core::sync::records::{EntityKind, Mismatch, Resolution, Status, SyncRecord};
    use sat_core::table::ProviderTable;

    use super::Decision;
    use crate::{App, Command, Event, Query, Reply};

    #[test]
    fn records_are_decided_by_command_and_the_plan_follows() {
        let list = Modlist::new(vec![Entry {
            name: "A".into(),
            kind: EntryKind::Mod,
            enabled: true,
        }]);
        let mut a = App::new(
            Session::new(
                OrderBook::from_modlist(&list),
                Arc::new(ProviderTable::build(&[])),
            ),
            Registry::with_builtins(),
        );
        a.set_sync(
            vec![SyncRecord {
                fingerprint: "fs_created::mod::new".into(),
                entity: EntityKind::Mod,
                mismatch: Mismatch::FsCreated {
                    fs_name: "New".into(),
                },
                status: Status::Pending,
                resolution: None,
            }],
            vec!["A".into(), "New".into()],
        );
        let plan = |a: &mut App| {
            let Reply::Sync { records, plan, .. } = a.query(Query::Sync).unwrap() else {
                panic!()
            };
            (records, plan)
        };
        let (records, p) = plan(&mut a);
        assert_eq!(
            records[0].answers,
            [Resolution::AdoptToProject, Resolution::MoveToDeleted]
        );
        assert!(p.blocked());
        // an answer that does not fit is refused
        let bad = a.command(Command::Decide {
            fingerprint: "fs_created::mod::new".into(),
            decision: Decision::Confirm {
                resolution: Resolution::CreateDiskFolder,
            },
        });
        assert!(bad.is_err());
        let ev = a
            .command(Command::Decide {
                fingerprint: "fs_created::mod::new".into(),
                decision: Decision::Confirm {
                    resolution: Resolution::AdoptToProject,
                },
            })
            .unwrap();
        assert_eq!(ev, Event::Decided);
        let (_, p) = plan(&mut a);
        assert!(!p.blocked());
        assert_eq!(p.canon_after.entries.len(), 2);
        assert_eq!(a.sync_records()[0].status, Status::Confirmed);
    }

    #[derive(Debug)]
    struct Fake;
    impl crate::PlanSource for Fake {
        fn full_plan(
            &self,
            project: &sat_core::project::Project,
            _: &[String],
        ) -> (sat_core::sync::plan::Plan, crate::PlanExtras) {
            // the shell sees the project as the window has it now: its records with their decisions
            let disk = ["A".to_string(), "New".to_string()];
            let mut p = sat_core::sync::plan::plan(&project.canon, &project.sync, &disk);
            p.ignored = project.sync.len();
            (
                p,
                crate::PlanExtras {
                    notes: vec![format!("{} entries", project.canon.entries.len())],
                    ..Default::default()
                },
            )
        }
    }

    #[test]
    fn with_a_build_the_plan_is_the_shells_full_plan() {
        let list = Modlist::new(vec![Entry {
            name: "A".into(),
            kind: EntryKind::Mod,
            enabled: true,
        }]);
        let mut a = App::new(
            Session::new(
                OrderBook::from_modlist(&list),
                Arc::new(ProviderTable::build(&[])),
            ),
            Registry::with_builtins(),
        );
        // no project, no source: the canon's plan, no extras
        let Reply::Sync { extras, .. } = a.query(Query::Sync).unwrap() else {
            panic!()
        };
        assert!(extras.is_none());
        let mut project = sat_core::project::Project::import(list);
        project.sync.push(SyncRecord {
            fingerprint: "fs_created::mod::new".into(),
            entity: EntityKind::Mod,
            mismatch: Mismatch::FsCreated {
                fs_name: "New".into(),
            },
            status: Status::Pending,
            resolution: None,
        });
        a.load_project(&project, vec!["A".into(), "New".into()]);
        a.set_plan_source(Arc::new(Fake));
        a.command(Command::Decide {
            fingerprint: "fs_created::mod::new".into(),
            decision: Decision::Confirm {
                resolution: Resolution::AdoptToProject,
            },
        })
        .unwrap();
        let Reply::Sync { plan, extras, .. } = a.query(Query::Sync).unwrap() else {
            panic!()
        };
        assert_eq!(extras.unwrap().notes, ["1 entries"]);
        assert!(
            !plan.blocked(),
            "the decision made in the window reached the shell: {:?}",
            plan.blockers
        );
        assert_eq!(plan.ignored, 1);
        // the project as it is now carries the window's decision
        assert_eq!(a.project().unwrap().sync[0].status, Status::Confirmed);
    }
}
