//! The project: canon of the build (ADR 0006). @sat:project
//!
//! The project — not `modlist.txt` — is the source of truth for order, enabled flags and separators. The build's
//! folders and the manager's files are *sides* compared against it:
//! - the mods folder → [`Project::observe`] → sync records ([`crate::sync::records`]);
//! - the manager's list → [`Project::manager_diff`] → a visible difference, never a silent replacement
//!   (the monolith's `pendingOrder` trap, NG-D001).
//!
//! The on-disk container is not decided yet (ADR 0007); until then the type is serde-serializable with a schema
//! number and callers store it as they see fit.
//!
//! Owns: the project: canon + signatures + sync records + decoration; observe and manager diff.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::canon::{EntryKind, Modlist};
#[cfg(test)]
use crate::order::MAIN;
use crate::order::{Decor, DecorIssue, OrderBook};
use crate::sync::records::{DiskFolder, EntityKind, ProjectEntity, Reconcile, SyncRecord, reconcile};

/// Version of the serialized shape. Bump with a migration when fields change meaning.
pub const PROJECT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    /// Order, enabled flags, separators — the canon.
    pub canon: Modlist,
    /// Content signature of each entity's folder the last time it matched (key: entry name, case-folded).
    /// Lets the next reconcile recognise a renamed folder by its content.
    pub signatures: BTreeMap<String, u64>,
    /// Current sync records with the user's decisions.
    pub sync: Vec<SyncRecord>,
    /// Colours, logical groups, collapsed separators (ADR 0008). Absent in older files.
    #[serde(default, skip_serializing_if = "Decor::is_empty")]
    pub decor: Decor,
}

/// How the manager's list differs from the project canon.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagerDiff {
    /// In the manager's list, not in the project.
    pub only_manager: Vec<String>,
    /// In the project, not in the manager's list.
    pub only_project: Vec<String>,
    /// Common entries with a different enabled flag.
    pub enabled_differs: Vec<String>,
    /// Common entries are in a different relative order.
    pub order_differs: bool,
}

impl ManagerDiff {
    pub fn is_empty(&self) -> bool {
        self.only_manager.is_empty()
            && self.only_project.is_empty()
            && self.enabled_differs.is_empty()
            && !self.order_differs
    }
}

fn fold(name: &str) -> String {
    name.to_lowercase()
}

impl Project {
    /// First open of a build: the canon is taken from the manager's list as is.
    pub fn import(list: Modlist) -> Self {
        Self {
            schema: PROJECT_SCHEMA,
            canon: list,
            signatures: BTreeMap::new(),
            sync: Vec::new(),
            decor: Decor::default(),
        }
    }

    // @sat:project.book
    /// The live book of this project: canon plus decoration. Decoration that no longer fits is reported.
    pub fn book(&self) -> (OrderBook, Vec<DecorIssue>) {
        let mut book = OrderBook::from_modlist(&self.canon);
        let issues = if self.decor.is_empty() {
            Vec::new()
        } else {
            book.apply_decor(&self.decor).1
        };
        (book, issues)
    }

    /// Takes canon and decoration back from a book after editing.
    pub fn commit_book(&mut self, book: &OrderBook) {
        self.canon = book.to_modlist();
        self.decor = book.decor();
    }

    /// Mods and separators of the canon as reconcile entities (foreign manager entries are not folders).
    pub fn entities(&self) -> Vec<ProjectEntity> {
        self.canon
            .entries
            .iter()
            .filter(|e| e.kind != EntryKind::Foreign)
            .map(|e| ProjectEntity {
                name: e.name.clone(),
                kind: if e.kind == EntryKind::Separator {
                    EntityKind::Separator
                } else {
                    EntityKind::Mod
                },
                signature: self.signatures.get(&fold(&e.name)).copied(),
            })
            .collect()
    }

    // @sat:project.observe
    /// Compares the canon with the mods folder: stores the records (keeping decisions) and records the content
    /// signature of every matched entity for the next run. The canon itself is not changed.
    pub fn observe(&mut self, disk: &[DiskFolder]) -> Reconcile {
        let r = reconcile(&self.entities(), disk, &self.sync);
        let sig: HashMap<&str, u64> = disk
            .iter()
            .filter_map(|d| d.signature.map(|s| (d.name.as_str(), s)))
            .collect();
        for (project_name, folder) in &r.matched {
            match sig.get(folder.as_str()) {
                Some(&s) => {
                    self.signatures.insert(fold(project_name), s);
                }
                None => {
                    self.signatures.remove(&fold(project_name));
                }
            }
        }
        // Signatures of entities no longer in the canon are dropped.
        let live: HashSet<String> = self.canon.entries.iter().map(|e| fold(&e.name)).collect();
        self.signatures.retain(|k, _| live.contains(k));
        self.sync = r.records.clone();
        r
    }

    // @sat:project.manager-diff
    /// Difference between the canon and the manager's list (e.g. the order was changed in MO2).
    pub fn manager_diff(&self, manager: &Modlist) -> ManagerDiff {
        let index = |l: &Modlist| -> HashMap<String, usize> {
            l.entries
                .iter()
                .enumerate()
                .map(|(i, e)| (fold(&e.name), i))
                .collect()
        };
        let (pi, mi) = (index(&self.canon), index(manager));
        let mut d = ManagerDiff::default();
        for e in &manager.entries {
            if !pi.contains_key(&fold(&e.name)) {
                d.only_manager.push(e.name.clone());
            }
        }
        let mut common_in_manager_order = Vec::new();
        for e in &self.canon.entries {
            match mi.get(&fold(&e.name)) {
                None => d.only_project.push(e.name.clone()),
                Some(&j) => {
                    let m = &manager.entries[j];
                    if m.enabled != e.enabled && e.kind == EntryKind::Mod {
                        d.enabled_differs.push(e.name.clone());
                    }
                    common_in_manager_order.push(j);
                }
            }
        }
        d.order_differs = common_in_manager_order.windows(2).any(|w| w[0] > w[1]);
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::Entry;
    use crate::sync::records::{Mismatch, Status};

    fn list(spec: &[&str]) -> Modlist {
        Modlist::new(
            spec.iter()
                .map(|s| {
                    let (kind, enabled, name) = match s.chars().next() {
                        Some('#') => (EntryKind::Separator, false, format!("{}_separator", &s[1..])),
                        Some('-') => (EntryKind::Mod, false, s[1..].to_string()),
                        Some('*') => (EntryKind::Foreign, true, s[1..].to_string()),
                        _ => (EntryKind::Mod, true, s.to_string()),
                    };
                    Entry { name, kind, enabled }
                })
                .collect(),
        )
    }
    fn d(name: &str, sig: Option<u64>) -> DiskFolder {
        DiskFolder {
            name: name.into(),
            signature: sig,
        }
    }

    #[test]
    fn import_then_rename_is_found_on_next_run() {
        let mut p = Project::import(list(&["#UI", "SkyUI", "Old Name", "*DLC"]));
        // run 1: everything matches; signatures are recorded (separator folder has none)
        let r1 = p.observe(&[
            d("UI_separator", None),
            d("SkyUI", Some(1)),
            d("Old Name", Some(42)),
        ]);
        assert!(r1.records.is_empty());
        assert_eq!(p.signatures.get("old name"), Some(&42));
        assert!(!p.signatures.contains_key("ui_separator"));
        // run 2: the folder was renamed on disk
        let r2 = p.observe(&[
            d("UI_separator", None),
            d("SkyUI", Some(1)),
            d("New Name", Some(42)),
        ]);
        assert_eq!(r2.records.len(), 1);
        assert!(matches!(
            &r2.records[0].mismatch,
            Mismatch::FsRenamed { project_name, fs_name } if project_name == "Old Name" && fs_name == "New Name"
        ));
        // canon untouched, record stored with its decision slot
        assert!(p.canon.index_of("Old Name").is_some());
        assert_eq!(p.sync[0].status, Status::Pending);
    }

    #[test]
    fn decisions_persist_through_observe() {
        let mut p = Project::import(list(&["A"]));
        p.observe(&[d("A", Some(1)), d("Extra", Some(2))]);
        p.sync[0].status = Status::Ignored;
        let r = p.observe(&[d("A", Some(1)), d("Extra", Some(2))]);
        assert_eq!(r.records[0].status, Status::Ignored);
        assert!(!r.blocked());
    }

    #[test]
    fn manager_diff_is_visible_not_applied() {
        let p = Project::import(list(&["A", "B", "-C", "D"]));
        let manager = list(&["B", "A", "C", "E"]);
        let diff = p.manager_diff(&manager);
        assert_eq!(diff.only_manager, vec!["E"]);
        assert_eq!(diff.only_project, vec!["D"]);
        assert_eq!(diff.enabled_differs, vec!["C"]);
        assert!(diff.order_differs);
        assert!(p.manager_diff(&p.canon).is_empty());
        // the canon is still the project's
        assert_eq!(p.canon.entries[0].name, "A");
    }

    #[test]
    fn book_round_trip_keeps_decoration() {
        let mut p = Project::import(list(&["#UI", "SkyUI", "A", "B", "C"]));
        let (mut book, issues) = p.book();
        assert!(issues.is_empty());
        let (a, b) = (book.key_of("A").unwrap(), book.key_of("B").unwrap());
        book.create_group("Pack", &[a, b]).unwrap();
        book.set_color(&[a], Some(3)).unwrap();
        book.move_keys(&[book.key_of("C").unwrap()], Some(a)).unwrap();
        p.commit_book(&book);
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        let (book2, issues) = back.book();
        assert!(issues.is_empty());
        // keys are renumbered by the rebuild; everything else is the same
        let strip = |b: &OrderBook| {
            b.rows(MAIN, 0, 10)
                .into_iter()
                .map(|r| (r.pos, r.kind, r.name.to_string(), r.color, r.size, r.depth))
                .collect::<Vec<_>>()
        };
        assert_eq!(strip(&book2), strip(&book));
        // files written before decoration existed still load
        let old = r#"{"schema":1,"canon":{"entries":[]},"signatures":{},"sync":[]}"#;
        assert!(serde_json::from_str::<Project>(old).unwrap().decor.is_empty());
    }

    #[test]
    fn serde_round_trip() {
        let mut p = Project::import(list(&["#UI", "SkyUI"]));
        p.observe(&[d("SkyUI", Some(7)), d("Stray", None)]);
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert_eq!(back.schema, PROJECT_SCHEMA);
    }
}
