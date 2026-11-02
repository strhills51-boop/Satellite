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
//! Owns: the project: canon + signatures + sync records + decoration + conflict decisions; observe and manager diff.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::canon::{EntryKind, Modlist};
use crate::decisions::ChainDecisions;
use crate::entity::{Entities, EntityId};
#[cfg(test)]
use crate::order::MAIN;
use crate::order::{Decor, DecorIssue, OrderBook};
use crate::sync::records::{DiskFolder, EntityKind, ProjectEntity, Reconcile, SyncRecord, reconcile};

/// Version of the serialized shape. Bump with a migration when fields change meaning.
/// 2: entities with stable ids; decoration and content signatures keyed by entity id (`@sat:entity`).
pub const PROJECT_SCHEMA: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub schema: u32,
    /// Order, enabled flags, separators — the canon.
    pub canon: Modlist,
    /// Every entity of the canon with its stable id (`@sat:entity`); sections below are keyed by id.
    #[serde(default)]
    pub entities: Entities,
    /// Content signature of each entity's folder the last time it matched (key: entity id).
    /// Lets the next reconcile recognise a renamed folder by its content.
    pub signatures: BTreeMap<String, u64>,
    /// Current sync records with the user's decisions.
    pub sync: Vec<SyncRecord>,
    /// Colours, logical groups, collapsed separators, styles, marks (ADR 0008, 0012), keyed by entity id.
    #[serde(default, skip_serializing_if = "Decor::is_empty")]
    pub decor: Decor,
    /// Conflict decisions: settled chains by content signature. Absent in older files.
    #[serde(default, skip_serializing_if = "ChainDecisions::is_empty")]
    pub decisions: ChainDecisions,
    /// The plugin part of the build (`@sat:plugins.canon`): load order, checked marks as last seen in the
    /// manager's files, and the plugins the user hid — so the plugin list works without the build.
    #[serde(default)]
    pub plugins: crate::plugins::PluginCanon,
    /// Read from early schema-2 files only; folded into `plugins.blacklist` on load.
    #[serde(default, skip_serializing)]
    pub plugin_blacklist: Vec<String>,
    /// Paths that are no conflict (`@sat:ignore`). Absent in older files.
    #[serde(default, skip_serializing_if = "crate::ignore::IgnoreRules::is_empty")]
    pub conflict_ignore: crate::ignore::IgnoreRules,
    /// Tag collections with colours (`@sat:tags`); the built-in ones when absent.
    #[serde(default, skip_serializing_if = "is_default_catalog")]
    pub tags: crate::tags::TagCatalog,
    /// What the user wants done to the build, fulfilled by a commit (`@sat:intent`): planned mods.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<crate::intent::Intent>,
    /// Fields this build does not know (written by a newer one or an extension): kept as they are and written
    /// back, so opening a project never loses another program's data (ADR 0007 §Модель).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

fn is_default_catalog(c: &crate::tags::TagCatalog) -> bool {
    *c == crate::tags::TagCatalog::default()
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
        let mut entities = Entities::default();
        entities.follow(&list);
        Self {
            schema: PROJECT_SCHEMA,
            canon: list,
            entities,
            signatures: BTreeMap::new(),
            sync: Vec::new(),
            decor: Decor::default(),
            decisions: ChainDecisions::default(),
            plugins: Default::default(),
            plugin_blacklist: Vec::new(),
            conflict_ignore: Default::default(),
            tags: Default::default(),
            intents: Vec::new(),
            extra: BTreeMap::new(),
        }
    }

    // @sat:project.load
    /// Reads a project file of any known schema; older ones are migrated (1 → 2: entities get ids, name-keyed
    /// sections are re-keyed by id).
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        Self::from_value(serde_json::from_slice(bytes).map_err(|e| e.to_string())?)
    }

    /// [`Project::from_json`] from a parsed tree (the `.stll` file joins its sections into one, `@sat:stll.file`).
    pub fn from_value(v: serde_json::Value) -> Result<Self, String> {
        let mut p: Project = serde_json::from_value(v).map_err(|e| e.to_string())?;
        for name in std::mem::take(&mut p.plugin_blacklist) {
            p.plugins.set_blacklisted(&name, true);
        }
        match p.schema {
            1 => {
                p.entities.follow(&p.canon);
                let by_name = |k: &str| p.entities.id_of(k).map(|i| i.0.clone());
                p.decor = p.decor.rekey(by_name).0;
                let signatures = std::mem::take(&mut p.signatures);
                for (name, sig) in signatures {
                    if let Some(id) = p.entities.id_of(&name) {
                        p.signatures.insert(id.0.clone(), sig);
                    }
                }
                p.schema = PROJECT_SCHEMA;
            }
            PROJECT_SCHEMA => p.entities.follow(&p.canon),
            s => {
                return Err(format!(
                    "project schema {s} is newer than this build reads ({PROJECT_SCHEMA})"
                ));
            }
        }
        Ok(p)
    }

    pub fn to_json(&self) -> Result<Vec<u8>, String> {
        serde_json::to_vec_pretty(self).map_err(|e| e.to_string())
    }

    pub fn to_value(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|e| e.to_string())
    }

    // @sat:project.book
    /// The live book of this project: canon plus decoration (re-keyed from ids to the names the book uses).
    /// Decoration that no longer fits is reported.
    /// (name, entity id) of every entity: the member keys of a session (`@sat:session.member-keys`), so chain
    /// decisions follow the entity through a rename.
    pub fn member_keys(&self) -> impl Iterator<Item = (String, String)> + '_ {
        self.entities.iter().map(|r| (r.name.clone(), r.id.0.clone()))
    }

    pub fn book(&self) -> (OrderBook, Vec<DecorIssue>) {
        let mut book = OrderBook::from_modlist(&self.canon);
        let issues = if self.decor.is_empty() {
            Vec::new()
        } else {
            let names = self.decor.rekey(|id| {
                self.entities
                    .name_of(&crate::entity::EntityId(id.to_string()))
                    .map(str::to_string)
            });
            book.apply_decor(&names.0).1
        };
        (book, issues)
    }

    /// Takes canon and decoration back from a book after editing (decoration re-keyed by entity id).
    pub fn commit_book(&mut self, book: &OrderBook) {
        self.canon = book.to_modlist();
        self.entities.follow(&self.canon);
        let entities = &self.entities;
        self.decor = book.decor().rekey(|n| entities.id_of(n).map(|i| i.0.clone())).0;
    }

    // @sat:project.apply-plan
    /// Applies a committed plan to the project: renames keep the entity (and its decoration, marks, signature),
    /// adopted folders become entities, removed ones leave; settled records are closed.
    pub fn apply_plan(&mut self, plan: &crate::sync::plan::Plan) {
        use crate::sync::plan::ProjectOp;
        for op in &plan.project_ops {
            match op {
                ProjectOp::Rename { from, to } => {
                    self.entities.rename(from, to);
                }
                ProjectOp::Adopt { .. } => {}
                ProjectOp::Remove { name } => {
                    if let Some(id) = self.entities.remove(name) {
                        self.signatures.remove(&id.0);
                    }
                }
            }
        }
        self.canon = plan.canon_after.clone();
        self.entities.follow(&self.canon);
        self.sync.retain(|r| !plan.settles.contains(&r.fingerprint));
        // installed planned mods: their intents are fulfilled
        for op in &plan.disk_ops {
            if let crate::sync::plan::DiskOp::Install { name, .. } = op
                && let Some(id) = self.entities.id_of(name)
            {
                for i in self.intents.iter_mut().filter(|i| &i.entity == id && i.pending()) {
                    i.status = crate::intent::IntentStatus::Done;
                }
            }
        }
        // intents of entities that left the project go with them
        let live: HashSet<&EntityId> = self.entities.iter().map(|e| &e.id).collect();
        self.intents.retain(|i| live.contains(&i.entity));
    }

    // @sat:intent.planned
    /// Planned mods: canon entries with a pending install, in canon order.
    pub fn planned(&self) -> Vec<crate::intent::Planned> {
        self.canon
            .entries
            .iter()
            .filter_map(|e| {
                let id = self.entities.id_of(&e.name)?;
                let (source, fomod) = self.intents.iter().find(|i| &i.entity == id)?.pending_install()?;
                Some(crate::intent::Planned {
                    name: e.name.clone(),
                    source: source.clone(),
                    fomod: fomod.cloned(),
                })
            })
            .collect()
    }

    /// Adds a planned mod: a new canon entry after `after` (at the end when `None` or unknown) with a pending
    /// install from `source`. `Err` if the name is taken.
    pub fn plan_install(
        &mut self,
        name: &str,
        source: crate::intent::Source,
        fomod: Option<Vec<String>>,
        after: Option<&str>,
        enabled: bool,
    ) -> Result<EntityId, String> {
        if self.entities.id_of(name).is_some() {
            return Err(format!("{name}: already in the project"));
        }
        let at = after
            .and_then(|a| {
                self.canon
                    .entries
                    .iter()
                    .position(|e| e.name.eq_ignore_ascii_case(a))
            })
            .map_or(self.canon.entries.len(), |i| i + 1);
        self.canon.entries.insert(
            at,
            crate::canon::Entry {
                name: name.to_string(),
                kind: EntryKind::Mod,
                enabled,
            },
        );
        self.entities.follow(&self.canon);
        let id = self.entities.id_of(name).cloned().expect("just added");
        self.intents
            .push(crate::intent::Intent::install(id.clone(), source, fomod));
        Ok(id)
    }

    /// Mods and separators of the canon as reconcile entities (foreign manager entries are not folders).
    pub fn entities(&self) -> Vec<ProjectEntity> {
        // a planned mod has no folder yet by design: that is no mismatch
        let planned: HashSet<String> = self
            .planned()
            .into_iter()
            .map(|p| p.name.to_lowercase())
            .collect();
        self.canon
            .entries
            .iter()
            .filter(|e| e.kind != EntryKind::Foreign && !planned.contains(&e.name.to_lowercase()))
            .map(|e| ProjectEntity {
                name: e.name.clone(),
                kind: if e.kind == EntryKind::Separator {
                    EntityKind::Separator
                } else {
                    EntityKind::Mod
                },
                signature: self
                    .entities
                    .id_of(&e.name)
                    .and_then(|id| self.signatures.get(&id.0))
                    .copied(),
            })
            .collect()
    }

    // @sat:project.observe
    /// Compares the canon with the mods folder: stores the records (keeping decisions) and records the content
    /// signature of every matched entity for the next run. The canon itself is not changed.
    pub fn observe(&mut self, disk: &[DiskFolder]) -> Reconcile {
        // a planned mod whose folder is there was installed (outside Satellite, or by a commit): fulfilled
        let present: HashSet<String> = disk.iter().map(|d| d.name.to_lowercase()).collect();
        for crate::intent::Planned { name, .. } in self.planned() {
            if present.contains(&name.to_lowercase())
                && let Some(id) = self.entities.id_of(&name).cloned()
            {
                for i in self.intents.iter_mut().filter(|i| i.entity == id && i.pending()) {
                    i.status = crate::intent::IntentStatus::Done;
                }
            }
        }
        let r = reconcile(&self.entities(), disk, &self.sync);
        let sig: HashMap<&str, u64> = disk
            .iter()
            .filter_map(|d| d.signature.map(|s| (d.name.as_str(), s)))
            .collect();
        self.entities.follow(&self.canon);
        for (project_name, folder) in &r.matched {
            let Some(id) = self.entities.id_of(project_name).map(|i| i.0.clone()) else {
                continue;
            };
            match sig.get(folder.as_str()) {
                Some(&s) => {
                    self.signatures.insert(id, s);
                }
                None => {
                    self.signatures.remove(&id);
                }
            }
        }
        // Signatures of entities no longer in the canon are dropped.
        let live: HashSet<String> = self.entities.iter().map(|e| e.id.0.clone()).collect();
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
        let id = |p: &Project, n: &str| p.entities.id_of(n).unwrap().0.clone();
        assert_eq!(p.signatures.get(&id(&p, "Old Name")), Some(&42));
        assert!(!p.signatures.contains_key(&id(&p, "UI_separator")));
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
    fn fields_of_a_newer_build_survive_a_round_trip() {
        let mut v = serde_json::to_value(Project::import(Modlist::new(Vec::new()))).unwrap();
        v["journal_hint"] = serde_json::json!({"last": 3});
        let p = Project::from_value(v).unwrap();
        assert_eq!(p.extra.get("journal_hint"), Some(&serde_json::json!({"last": 3})));
        let back: serde_json::Value = serde_json::from_slice(&p.to_json().unwrap()).unwrap();
        assert_eq!(back["journal_hint"]["last"], 3);
    }

    #[test]
    fn a_planned_mod_makes_no_sync_record_until_its_folder_appears() {
        use crate::intent::{IntentStatus, Source};
        let mut p = Project::import(Modlist::new(vec![Entry {
            name: "A".into(),
            kind: EntryKind::Mod,
            enabled: true,
        }]));
        let src = Source::Downloads {
            file: "B-123.zip".into(),
            size: Some(5),
        };
        p.plan_install("B", src.clone(), None, Some("nobody"), true)
            .unwrap();
        assert!(
            p.plan_install("b", src.clone(), None, None, true).is_err(),
            "names are taken any case"
        );
        assert_eq!(p.canon.entries[1].name, "B");
        assert_eq!(p.planned()[0].name, "B");
        assert_eq!(p.planned()[0].source, src);
        let folder = |n: &str| DiskFolder {
            name: n.into(),
            signature: None,
        };
        let r = p.observe(&[folder("A")]);
        assert!(r.records.is_empty(), "{:?}", r.records);
        // the mod was installed by hand: the intent is fulfilled, the mod is an ordinary one
        let r = p.observe(&[folder("A"), folder("B")]);
        assert!(r.records.is_empty());
        assert_eq!(p.intents[0].status, IntentStatus::Done);
        assert!(p.planned().is_empty());
        // the record keeps its shape through the project file
        let back = Project::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(back.intents, p.intents);
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

    #[test]
    fn a_committed_rename_keeps_the_entity_and_its_decoration() {
        use crate::order::Marks;
        use crate::sync::plan::plan;
        use crate::sync::records::Resolution;
        let mut p = Project::import(list(&["A", "Old"]));
        // the user tags and colours "Old" in a book; the project keeps it by id
        let (mut book, _) = p.book();
        let k = book.key_of("Old").unwrap();
        book.set_color(&[k], Some(7)).unwrap();
        book.patch_marks(&[k], |mut m: Marks| {
            m.tags.push("keep me".into());
            m
        })
        .unwrap();
        p.commit_book(&book);
        let id = p.entities.id_of("Old").unwrap().clone();
        assert!(p.decor.colors.contains_key(&id.0), "stored by id, not by name");
        // the folder was renamed on disk; the user accepts the new name; the commit applies it
        p.observe(&[d("A", Some(1)), d("Old", Some(9))]);
        p.observe(&[d("A", Some(1)), d("New", Some(9))]);
        p.sync[0].status = Status::Confirmed;
        p.sync[0].resolution = Some(Resolution::AcceptFsName);
        let pl = plan(&p.canon, &p.sync, &["A".into(), "New".into()]);
        assert!(!pl.blocked(), "{:?}", pl.blockers);
        p.apply_plan(&pl);
        assert_eq!(p.entities.id_of("New"), Some(&id), "same entity");
        assert_eq!(p.signatures.get(&id.0), Some(&9), "its signature too");
        assert!(p.sync.is_empty());
        let (book, issues) = p.book();
        assert!(issues.is_empty());
        let k = book.key_of("New").unwrap();
        assert_eq!(book.marks(k).unwrap().tags, ["keep me"]);
    }

    #[test]
    fn a_schema_1_file_is_migrated_to_ids() {
        let v1 = r#"{"schema":1,"canon":{"entries":[{"name":"A","kind":"Mod","enabled":true}]},
            "signatures":{"a":5},"sync":[],"decor":{"colors":{"A":3},"groups":[],"collapsed":[],"muted":[]}}"#;
        let p = Project::from_json(v1.as_bytes()).unwrap();
        assert_eq!(p.schema, PROJECT_SCHEMA);
        let id = p.entities.id_of("A").unwrap().0.clone();
        assert_eq!(
            (p.signatures.get(&id), p.decor.colors.get(&id)),
            (Some(&5), Some(&3))
        );
        // and survives a round trip with the same ids
        let back = Project::from_json(&p.to_json().unwrap()).unwrap();
        assert_eq!(back.entities.id_of("A").map(|i| &i.0), Some(&id));
        assert!(
            Project::from_json(br#"{"schema":99,"canon":{"entries":[]},"signatures":{},"sync":[]}"#).is_err()
        );
    }

    #[test]
    fn an_early_blacklist_field_folds_into_the_plugin_section() {
        let early = r#"{"schema":2,"canon":{"entries":[]},"signatures":{},"sync":[],"plugin_blacklist":["Hidden.esp"]}"#;
        let p = Project::from_json(early.as_bytes()).unwrap();
        assert_eq!(p.plugins.blacklist, ["Hidden.esp"]);
        let json = String::from_utf8(p.to_json().unwrap()).unwrap();
        assert!(
            !json.contains("plugin_blacklist"),
            "written only in the new place"
        );
    }
}
