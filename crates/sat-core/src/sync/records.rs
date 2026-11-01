//! Sync records and the disk ↔ project reconcile (ADR 0006). @sat:sync.records
//!
//! A pure function: the caller supplies what the project says and what the mods folder holds; the result is the
//! set of mismatches as records. Nothing here touches the disk or the project — resolving a record and committing
//! the plan are separate steps (NG-D003).
//!
//! Monolith reference: `buildSyncReconcileResult`, `upsertSyncRecord` (`docs/semantic/monolith/11-sync.md`).
//! Deliberate differences, each fixing a monolith defect found in the conspectus:
//! - a rename is **one** record, not `fs_created` + `fs_deleted` + `fs_rename_candidate`;
//! - re-running the reconcile keeps the user's decision (status, resolution) on a record that is still valid;
//!   the monolith reset every disk-side record to `pending` on each run, so they could never be committed;
//! - the content signature does not include the folder name (`sat-scan::signature`), so renames can match at all;
//! - fingerprints use the exact case-folded name, not the normalized one, so distinct folders never merge;
//! - an exact (case-insensitive) name match wins before the normalized one, so `Mod-A` and `Mod A` on disk stay
//!   two folders instead of one silently shadowing the other.
//!
//! Owns: sync records and the pure disk ↔ project reconcile.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::names::{is_separator_folder, normalize_name};

/// What a record is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Mod,
    Separator,
}

impl EntityKind {
    fn of_folder(name: &str) -> Self {
        if is_separator_folder(name) {
            Self::Separator
        } else {
            Self::Mod
        }
    }
    fn tag(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::Separator => "separator",
        }
    }
}

/// A project entity the reconcile compares against: a mod or a separator (by its folder name, `X_separator`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEntity {
    pub name: String,
    pub kind: EntityKind,
    /// Content signature recorded the last time the folder was seen (for rename detection); `None` if never scanned.
    pub signature: Option<u64>,
}

/// A folder in the mods folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskFolder {
    pub name: String,
    /// Content signature of the folder (independent of its name).
    pub signature: Option<u64>,
}

/// The kind of mismatch.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Mismatch {
    /// Folder on disk the project does not know.
    FsCreated { fs_name: String },
    /// Project entity whose folder is gone.
    FsDeleted { project_name: String },
    /// Project entity whose folder is gone, and exactly one unknown folder has the same content.
    FsRenamed { project_name: String, fs_name: String },
}

/// User decision on a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Waiting for a decision; blocks a commit.
    #[default]
    Pending,
    /// Accepted; becomes an operation of the commit plan.
    Confirmed,
    /// Deliberately left as is; survives later reconciles while the mismatch persists.
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncRecord {
    /// Stable identity: same mismatch → same fingerprint across runs.
    pub fingerprint: String,
    pub entity: EntityKind,
    pub mismatch: Mismatch,
    pub status: Status,
    /// How a confirmed record is applied; interpreted by the commit planner (`@sat:sync.plan`).
    pub resolution: Option<Resolution>,
}

// @sat:sync.records.resolution
/// The decision matrix of the monolith (`11-sync.md` §4.4), one variant per allowed answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// `FsCreated`: take the folder into the project (a mod is appended enabled; a separator becomes a block).
    AdoptToProject,
    /// `FsCreated`: move the folder out of the mods folder into `deleted/<commit>` (never a hard delete).
    MoveToDeleted,
    /// `FsDeleted`: create an empty folder with the project's name.
    CreateDiskFolder,
    /// `FsDeleted`: take the entity out of the project.
    RemoveFromProject,
    /// `FsRenamed`: the project takes the folder's new name.
    AcceptFsName,
    /// `FsRenamed`: the folder is renamed back to the project's name.
    RenameDiskToProject,
}

impl Resolution {
    /// Whether this answer applies to that mismatch.
    pub fn fits(self, m: &Mismatch) -> bool {
        matches!(
            (self, m),
            (
                Self::AdoptToProject | Self::MoveToDeleted,
                Mismatch::FsCreated { .. }
            ) | (
                Self::CreateDiskFolder | Self::RemoveFromProject,
                Mismatch::FsDeleted { .. }
            ) | (
                Self::AcceptFsName | Self::RenameDiskToProject,
                Mismatch::FsRenamed { .. }
            )
        )
    }
}

impl SyncRecord {
    fn new(entity: EntityKind, mismatch: Mismatch) -> Self {
        let fingerprint = match &mismatch {
            Mismatch::FsCreated { fs_name } => format!("fs_created::{}::{}", entity.tag(), fp(fs_name)),
            Mismatch::FsDeleted { project_name } => {
                format!("fs_deleted::{}::{}", entity.tag(), fp(project_name))
            }
            Mismatch::FsRenamed {
                project_name,
                fs_name,
            } => format!(
                "fs_renamed::{}::{}::{}",
                entity.tag(),
                fp(project_name),
                fp(fs_name)
            ),
        };
        Self {
            fingerprint,
            entity,
            mismatch,
            status: Status::Pending,
            resolution: None,
        }
    }
}

fn key(name: &str) -> String {
    normalize_name(name)
}

/// Fingerprint key: the exact name, case-folded. Not the normalized name — distinct folders like `Mod A` and
/// `Mod-A` normalize to the same key and would merge into one record (a monolith defect).
fn fp(name: &str) -> String {
    name.to_lowercase()
}

/// Result of a reconcile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconcile {
    /// Current mismatches, in a deterministic order (renames, deleted, created; then by fingerprint).
    pub records: Vec<SyncRecord>,
    /// Project entities matched to a folder (project name → folder name), including case/format differences.
    pub matched: Vec<(String, String)>,
}

impl Reconcile {
    /// A commit is blocked while any record is pending.
    pub fn blocked(&self) -> bool {
        self.records.iter().any(|r| r.status == Status::Pending)
    }
    pub fn count(&self, status: Status) -> usize {
        self.records.iter().filter(|r| r.status == status).count()
    }
}

// @sat:sync.records.reconcile
/// Compares the project with the mods folder.
///
/// `previous` are the records from the last reconcile: a record whose mismatch still exists keeps its status and
/// resolution; records whose mismatch disappeared are dropped.
pub fn reconcile(project: &[ProjectEntity], disk: &[DiskFolder], previous: &[SyncRecord]) -> Reconcile {
    // 1. Exact (case-insensitive) matches, then normalized matches among the rest.
    let mut disk_taken = vec![false; disk.len()];
    let mut proj_taken = vec![false; project.len()];
    let mut matched = Vec::new();
    let mut by_lower: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, d) in disk.iter().enumerate() {
        by_lower.entry(d.name.to_lowercase()).or_default().push(i);
    }
    for (pi, p) in project.iter().enumerate() {
        if let Some(&di) = by_lower
            .get(&p.name.to_lowercase())
            .and_then(|v| v.iter().find(|&&i| !disk_taken[i]))
        {
            disk_taken[di] = true;
            proj_taken[pi] = true;
            matched.push((p.name.clone(), disk[di].name.clone()));
        }
    }
    let mut by_norm: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, d) in disk.iter().enumerate().filter(|(i, _)| !disk_taken[*i]) {
        by_norm.entry(key(&d.name)).or_default().push(i);
    }
    for pi in (0..project.len()).filter(|&i| !proj_taken[i]).collect::<Vec<_>>() {
        let p = &project[pi];
        let candidates = by_norm.get(&key(&p.name));
        // Ambiguous normalized match (two folders collapse to one key) is not a match.
        if let Some([di]) = candidates
            .map(|v| v.iter().filter(|&&i| !disk_taken[i]).copied().collect::<Vec<_>>())
            .as_deref()
        {
            disk_taken[*di] = true;
            proj_taken[pi] = true;
            matched.push((p.name.clone(), disk[*di].name.clone()));
        }
    }

    // 2. Renames: an unmatched entity and an unmatched folder of the same kind with equal content signature,
    //    each being the only one with that signature on its side.
    let unique = |sigs: Vec<(EntityKind, u64)>| {
        let mut count: HashMap<(EntityKind, u64), usize> = HashMap::new();
        for s in sigs {
            *count.entry(s).or_default() += 1;
        }
        count
    };
    let proj_sigs = unique(
        project
            .iter()
            .enumerate()
            .filter(|(i, _)| !proj_taken[*i])
            .filter_map(|(_, p)| p.signature.map(|s| (p.kind, s)))
            .collect(),
    );
    let disk_sigs = unique(
        disk.iter()
            .enumerate()
            .filter(|(i, _)| !disk_taken[*i])
            .filter_map(|(_, d)| d.signature.map(|s| (EntityKind::of_folder(&d.name), s)))
            .collect(),
    );
    let mut records = Vec::new();
    for pi in (0..project.len()).filter(|&i| !proj_taken[i]).collect::<Vec<_>>() {
        let p = &project[pi];
        let Some(sig) = p.signature else { continue };
        if proj_sigs.get(&(p.kind, sig)) != Some(&1) || disk_sigs.get(&(p.kind, sig)) != Some(&1) {
            continue;
        }
        let Some(di) = (0..disk.len()).find(|&i| {
            !disk_taken[i] && disk[i].signature == Some(sig) && EntityKind::of_folder(&disk[i].name) == p.kind
        }) else {
            continue;
        };
        disk_taken[di] = true;
        proj_taken[pi] = true;
        records.push(SyncRecord::new(
            p.kind,
            Mismatch::FsRenamed {
                project_name: p.name.clone(),
                fs_name: disk[di].name.clone(),
            },
        ));
    }

    // 3. The rest.
    for (_, p) in project.iter().enumerate().filter(|(i, _)| !proj_taken[*i]) {
        records.push(SyncRecord::new(
            p.kind,
            Mismatch::FsDeleted {
                project_name: p.name.clone(),
            },
        ));
    }
    for (_, d) in disk.iter().enumerate().filter(|(i, _)| !disk_taken[*i]) {
        records.push(SyncRecord::new(
            EntityKind::of_folder(&d.name),
            Mismatch::FsCreated {
                fs_name: d.name.clone(),
            },
        ));
    }

    // 4. Keep user decisions of records that still hold.
    let prev: HashMap<&str, &SyncRecord> = previous.iter().map(|r| (r.fingerprint.as_str(), r)).collect();
    for r in &mut records {
        if let Some(p) = prev.get(r.fingerprint.as_str()) {
            r.status = p.status;
            r.resolution = p.resolution;
        }
    }
    let order = |r: &SyncRecord| match r.mismatch {
        Mismatch::FsRenamed { .. } => 0,
        Mismatch::FsDeleted { .. } => 1,
        Mismatch::FsCreated { .. } => 2,
    };
    records.sort_by(|a, b| {
        order(a)
            .cmp(&order(b))
            .then_with(|| a.fingerprint.cmp(&b.fingerprint))
    });
    debug_assert_eq!(
        records
            .iter()
            .map(|r| &r.fingerprint)
            .collect::<HashSet<_>>()
            .len(),
        records.len(),
        "fingerprints are unique"
    );
    Reconcile { records, matched }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &str, sig: Option<u64>) -> ProjectEntity {
        ProjectEntity {
            name: name.into(),
            kind: EntityKind::of_folder(name),
            signature: sig,
        }
    }
    fn d(name: &str, sig: Option<u64>) -> DiskFolder {
        DiskFolder {
            name: name.into(),
            signature: sig,
        }
    }
    fn kinds(r: &Reconcile) -> Vec<String> {
        r.records.iter().map(|r| r.fingerprint.clone()).collect()
    }

    #[test]
    fn in_sync_has_no_records() {
        let r = reconcile(
            &[m("A", Some(1)), m("UI_separator", None)],
            &[d("a", Some(1)), d("UI_separator", None)],
            &[],
        );
        assert!(r.records.is_empty());
        assert_eq!(r.matched.len(), 2);
        assert!(!r.blocked());
    }

    #[test]
    fn created_and_deleted_with_kinds() {
        let r = reconcile(
            &[m("Gone", Some(9)), m("Old_separator", None)],
            &[d("New", Some(5)), d("Fresh_separator", None)],
            &[],
        );
        assert_eq!(
            kinds(&r),
            vec![
                "fs_deleted::mod::gone",
                "fs_deleted::separator::old_separator",
                "fs_created::mod::new",
                "fs_created::separator::fresh_separator",
            ]
        );
        assert!(r.blocked());
    }

    #[test]
    fn normalized_match_and_ambiguity() {
        // "Mod.A" matches "mod a" after normalization
        let r = reconcile(&[m("Mod.A", None)], &[d("mod a", None)], &[]);
        assert!(r.records.is_empty());
        // two folders collapsing to one key: exact one wins, the other is a new folder
        let r = reconcile(&[m("Mod-A", None)], &[d("Mod A", None), d("Mod-A", None)], &[]);
        assert_eq!(kinds(&r), vec!["fs_created::mod::mod a"]);
        // no exact match and two normalized candidates: not matched at all
        let r = reconcile(&[m("Mod_A", None)], &[d("Mod A", None), d("Mod-A", None)], &[]);
        assert_eq!(r.records.len(), 3);
    }

    #[test]
    fn rename_is_one_record_only_when_unique() {
        let r = reconcile(&[m("Old Name", Some(42))], &[d("New Name", Some(42))], &[]);
        assert_eq!(kinds(&r), vec!["fs_renamed::mod::old name::new name"]);
        // two folders with the same content: ambiguous → plain deleted + 2 created
        let r = reconcile(
            &[m("Old", Some(42))],
            &[d("Copy1", Some(42)), d("Copy2", Some(42))],
            &[],
        );
        assert_eq!(r.records.len(), 3);
        assert!(
            r.records
                .iter()
                .all(|x| !matches!(x.mismatch, Mismatch::FsRenamed { .. }))
        );
        // never-scanned entity cannot be matched by content
        let r = reconcile(&[m("Old", None)], &[d("New", Some(42))], &[]);
        assert_eq!(r.records.len(), 2);
        // a mod never pairs with a separator folder
        let r = reconcile(&[m("Old", Some(7))], &[d("X_separator", Some(7))], &[]);
        assert_eq!(r.records.len(), 2);
    }

    #[test]
    fn decisions_survive_reconcile_and_stale_records_drop() {
        let project = [m("Gone", Some(1))];
        let disk = [d("New", Some(2))];
        let mut first = reconcile(&project, &disk, &[]).records;
        first[0].status = Status::Ignored;
        first[1].status = Status::Confirmed;
        first[1].resolution = Some(Resolution::AdoptToProject);
        let again = reconcile(&project, &disk, &first);
        assert_eq!(again.records, first, "same mismatches keep decisions");
        assert!(!again.blocked());
        // the folder came back: the deleted record disappears, the created one stays decided
        let later = reconcile(&project, &[d("New", Some(2)), d("Gone", Some(1))], &first);
        assert_eq!(kinds(&later), vec!["fs_created::mod::new"]);
        assert_eq!(later.records[0].status, Status::Confirmed);
    }
}
