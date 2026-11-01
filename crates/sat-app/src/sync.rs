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

    /// The commit plan over the current canon (what `modlist.txt` would receive) and the decided records.
    pub(crate) fn commit_plan(&mut self) -> Plan {
        let canon = self.session.book().to_modlist();
        plan(&canon, &self.sync.records, &self.sync.disk)
    }
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
            let Reply::Sync { records, plan } = a.query(Query::Sync).unwrap() else {
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
}
