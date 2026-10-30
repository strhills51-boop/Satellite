//! External donor sessions (monolith `ExternalDonorSession`): another build's `modlist.txt` + mods folder,
//! matched against the project to prepare a migration.
//!
//! Pure functions: the caller supplies the donor modlist, the donor folder names and the project's mods.
//! Nothing here moves files — moving is a commit-plan operation behind `external_migration_pending` records.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::names::{NameKeys, is_separator_folder, loose_key, norm_key, raw_key};
use crate::canon::{EntryKind, Modlist};

/// Name lookup over folder names: exact (raw key) first, then any of the name keys. First folder wins.
#[derive(Debug, Default)]
pub struct FolderLookup {
    exact: HashMap<String, usize>,
    keyed: HashMap<String, usize>,
    names: Vec<String>,
}

impl FolderLookup {
    pub fn new(names: impl IntoIterator<Item = String>) -> Self {
        let mut l = Self::default();
        for (i, name) in names.into_iter().enumerate() {
            l.exact.entry(raw_key(&name)).or_insert(i);
            for k in NameKeys::of(&name).0 {
                l.keyed.entry(k).or_insert(i);
            }
            l.names.push(name);
        }
        l
    }

    /// Monolith `findExternalFolderForName`.
    pub fn find(&self, name: &str) -> Option<&str> {
        let i = self.exact.get(&raw_key(name)).copied().or_else(|| {
            NameKeys::of(name)
                .0
                .iter()
                .find_map(|k| self.keyed.get(k).copied())
        })?;
        Some(&self.names[i])
    }
}

/// A mod of the project, as a migration target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetMod {
    pub name: String,
    pub separator: String,
}

/// Monolith `buildExternalTargetModMatchIndex`.
#[derive(Debug, Default)]
pub struct TargetIndex {
    by_key: HashMap<String, Vec<usize>>,
    /// (norm, loose) per target, for `contains` matching.
    normalized: Vec<(String, String)>,
    pub targets: Vec<TargetMod>,
}

/// How a donor name relates to the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchStatus {
    /// Some name key equals a project mod's key.
    Exact,
    /// Normalized or loose names contain one another (both at least 5 chars).
    Contains,
    /// Not in the project.
    DonorOnly,
    /// Listed in the donor modlist, but no folder in the donor mods folder.
    FolderMissing,
}

const MIN_CONTAINS_LEN: usize = 5;

impl TargetIndex {
    pub fn new(targets: Vec<TargetMod>) -> Self {
        let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
        let mut normalized = Vec::with_capacity(targets.len());
        for (i, t) in targets.iter().enumerate() {
            for k in NameKeys::of(&t.name).0 {
                by_key.entry(k).or_default().push(i);
            }
            normalized.push((norm_key(&t.name), loose_key(&t.name)));
        }
        Self {
            by_key,
            normalized,
            targets,
        }
    }

    /// Project mods sharing a name key with `name` (monolith `getExternalExactTargetMatches`).
    pub fn exact_matches(&self, name: &str) -> Vec<&TargetMod> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for k in NameKeys::of(name).0 {
            for &i in self.by_key.get(&k).map_or(&[][..], Vec::as_slice) {
                if seen.insert(i) {
                    out.push(&self.targets[i]);
                }
            }
        }
        out
    }

    /// Monolith `getTargetModNameMatchStatus`.
    pub fn status(&self, name: &str) -> MatchStatus {
        if raw_key(name).is_empty() || self.targets.is_empty() {
            return MatchStatus::DonorOnly;
        }
        if !self.exact_matches(name).is_empty() {
            return MatchStatus::Exact;
        }
        let (qn, ql) = (norm_key(name), loose_key(name));
        let long = |s: &str| s.chars().count() >= MIN_CONTAINS_LEN;
        let contains = self.normalized.iter().any(|(tn, tl)| {
            (long(&qn) && long(tn) && (tn.contains(&qn) || qn.contains(tn.as_str())))
                || (long(&ql) && long(tl) && (tl.contains(&ql) || ql.contains(tl.as_str())))
        });
        if contains {
            MatchStatus::Contains
        } else {
            MatchStatus::DonorOnly
        }
    }
}

/// UI bucket of a donor mod (monolith `getExternalDonorCandidateType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateKind {
    /// Has a folder, not in the project: ready to migrate.
    Ready,
    /// Already in the project under a matching name.
    Exist,
    /// A similar name is in the project — needs a human look.
    Similar,
    /// In the donor modlist without a folder: cannot be migrated.
    Missing,
    /// Folder in the donor mods folder that the donor modlist does not mention.
    FolderOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub name: String,
    pub enabled: bool,
    pub source_separator: String,
    pub folder: Option<String>,
    pub has_modlist_entry: bool,
    pub status: MatchStatus,
    pub kind: CandidateKind,
}

/// Monolith `buildExternalDonorValidation`: how well a mods folder fits a modlist.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Validation {
    /// 0–100: 65 % recall (modlist mods with a folder) + 35 % precision (folders listed in the modlist).
    pub score: u32,
    pub matched: usize,
    pub modlist_total: usize,
    pub folder_total: usize,
    pub separator_folder_total: usize,
    pub modlist_only: Vec<String>,
    pub folder_only: Vec<String>,
}

impl Validation {
    /// `ok` ≥ 80, `warn` ≥ 50, else `bad` (monolith `getValidationToneByScore`).
    pub fn tone(&self) -> &'static str {
        match self.score {
            80.. => "ok",
            50.. => "warn",
            _ => "bad",
        }
    }
}

/// Donor mods with their separator, in display order; mods above the first separator belong to `GLOBAL`.
fn donor_mods(list: &Modlist) -> Vec<(String, bool, String)> {
    let mut sep = "GLOBAL".to_string();
    let mut out = Vec::new();
    for e in &list.entries {
        match e.kind {
            EntryKind::Separator => {
                sep = e
                    .name
                    .strip_suffix(crate::canon::SEPARATOR_SUFFIX)
                    .unwrap_or(&e.name)
                    .to_string();
            }
            EntryKind::Mod => out.push((e.name.clone(), e.enabled, sep.clone())),
            EntryKind::Foreign => {}
        }
    }
    out
}

pub fn validate(list: &Modlist, folders: &[String]) -> Validation {
    let mods = donor_mods(list);
    let mod_folders: Vec<String> = folders
        .iter()
        .filter(|f| !is_separator_folder(f))
        .cloned()
        .collect();
    let lookup = FolderLookup::new(mod_folders.iter().cloned());
    let modlist_keys: HashSet<String> = mods.iter().flat_map(|(n, ..)| NameKeys::of(n).0).collect();
    let modlist_only: Vec<String> = mods
        .iter()
        .filter(|(n, ..)| lookup.find(n).is_none())
        .map(|(n, ..)| n.clone())
        .collect();
    let matched = mods.len() - modlist_only.len();
    let folder_only: Vec<String> = mod_folders
        .iter()
        .filter(|f| !NameKeys::of(f).0.iter().any(|k| modlist_keys.contains(k)))
        .cloned()
        .collect();
    let ratio = |a: usize, b: usize| if b == 0 { 0.0 } else { a as f64 / b as f64 };
    let recall = ratio(matched, mods.len());
    let precision = ratio(matched, mod_folders.len());
    Validation {
        score: ((recall * 0.65 + precision * 0.35) * 100.0).round() as u32,
        matched,
        modlist_total: mods.len(),
        folder_total: mod_folders.len(),
        separator_folder_total: folders.len() - mod_folders.len(),
        modlist_only,
        folder_only,
    }
}

/// Monolith `buildExternalDonorCandidates`: every donor mod (by modlist, then folder-only) against the project.
pub fn candidates(list: &Modlist, folders: &[String], target: &TargetIndex) -> Vec<Candidate> {
    let mod_folders: Vec<String> = folders
        .iter()
        .filter(|f| !is_separator_folder(f))
        .cloned()
        .collect();
    let lookup = FolderLookup::new(mod_folders.iter().cloned());
    let mut used = HashSet::new();
    let mut out = Vec::new();
    for (name, enabled, sep) in donor_mods(list) {
        let folder = lookup.find(&name).map(str::to_string);
        let status = match &folder {
            Some(f) => {
                used.insert(raw_key(f));
                target.status(f)
            }
            None => MatchStatus::FolderMissing,
        };
        out.push(Candidate {
            kind: kind_of(folder.is_some(), true, status),
            name,
            enabled,
            source_separator: sep,
            folder,
            has_modlist_entry: true,
            status,
        });
    }
    for f in mod_folders {
        if used.contains(&raw_key(&f)) {
            continue;
        }
        let status = target.status(&f);
        out.push(Candidate {
            kind: kind_of(true, false, status),
            name: f.clone(),
            enabled: true,
            source_separator: "[folder only]".into(),
            folder: Some(f),
            has_modlist_entry: false,
            status,
        });
    }
    out
}

fn kind_of(has_folder: bool, has_entry: bool, status: MatchStatus) -> CandidateKind {
    if has_folder && !has_entry {
        return CandidateKind::FolderOnly;
    }
    match status {
        _ if !has_folder => CandidateKind::Missing,
        MatchStatus::FolderMissing => CandidateKind::Missing,
        MatchStatus::Exact => CandidateKind::Exist,
        MatchStatus::Contains => CandidateKind::Similar,
        MatchStatus::DonorOnly => CandidateKind::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::Entry;

    fn list(spec: &[&str]) -> Modlist {
        Modlist::new(
            spec.iter()
                .map(|s| match s.strip_prefix('#') {
                    Some(n) => Entry {
                        name: format!("{n}_separator"),
                        kind: EntryKind::Separator,
                        enabled: false,
                    },
                    None => Entry {
                        name: s.to_string(),
                        kind: EntryKind::Mod,
                        enabled: true,
                    },
                })
                .collect(),
        )
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn validation_score() {
        let l = list(&["Top", "#Ui", "SkyUI SE", "Missing Mod"]);
        let v = validate(&l, &s(&["Top", "SkyUI_SE", "Stray", "Ui_separator"]));
        assert_eq!(
            (
                v.matched,
                v.modlist_total,
                v.folder_total,
                v.separator_folder_total
            ),
            (2, 3, 3, 1)
        );
        assert_eq!(v.modlist_only, s(&["Missing Mod"]));
        assert_eq!(v.folder_only, s(&["Stray"]));
        // recall 2/3 * 0.65 + precision 2/3 * 0.35 = 66.7
        assert_eq!(v.score, 67);
        assert_eq!(v.tone(), "warn");
    }

    #[test]
    fn candidate_kinds() {
        let target = TargetIndex::new(vec![
            TargetMod {
                name: "SkyUI".into(),
                separator: "UI".into(),
            },
            TargetMod {
                name: "Immersive Armors".into(),
                separator: "Gear".into(),
            },
        ]);
        let l = list(&["Loose", "#Ui", "SkyUI", "Immersive Armors Patch", "Gone"]);
        let c = candidates(
            &l,
            &s(&["Loose", "skyui", "Immersive Armors Patch", "Extra"]),
            &target,
        );
        let kinds: Vec<(&str, CandidateKind, &str)> = c
            .iter()
            .map(|c| (c.name.as_str(), c.kind, c.source_separator.as_str()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                ("Loose", CandidateKind::Ready, "GLOBAL"),
                ("SkyUI", CandidateKind::Exist, "Ui"),
                ("Immersive Armors Patch", CandidateKind::Similar, "Ui"),
                ("Gone", CandidateKind::Missing, "Ui"),
                ("Extra", CandidateKind::FolderOnly, "[folder only]"),
            ]
        );
        assert_eq!(c[1].folder.as_deref(), Some("skyui"));
    }
}
