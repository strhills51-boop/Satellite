//! The commit plan: decided sync records → canon changes and checked disk operations. @sat:sync.plan
//!
//! Owns: turning the decided records of a reconcile into a plan — what changes in the canon, which folders are
//! moved, renamed or created and in what order — and every reason the plan must not run (blockers).
//! Not here: running the plan on disk and writing the profile (`@sat:mo2.commit`), the reconcile itself
//! (`@sat:sync.records`), the decision matrix (`@sat:sync.records.resolution`).
//!
//! A pure function (NG-D003 §7, `11-sync.md` §6 items 4–5), fixing what the monolith got wrong:
//! - the plan writes nothing into the project; building it twice gives the same plan;
//! - disk operations are ordered by dependency, not by record time: names are freed first (moves to `deleted/`,
//!   then renames whose target is another rename's source), creations last;
//! - every collision is found before the first operation: a missing source, a target that exists, two operations
//!   into one name, a rename cycle — the monolith discovered these half-way through and left the build mixed;
//! - an unknown or unfitting answer is a blocker, never a silent "applied".

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use super::records::{EntityKind, Mismatch, Resolution, Status, SyncRecord};
use crate::canon::{Entry, EntryKind, Modlist};

/// A change of the canon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProjectOp {
    /// A folder taken into the project: appended at the end of the list (a mod enabled).
    Adopt {
        name: String,
        kind: EntityKind,
    },
    Remove {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
}

/// An operation on the mods folder, in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum DiskOp {
    /// Moved into `deleted/<commit>/` (reversible).
    ToDeleted {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Create {
        name: String,
        kind: EntityKind,
    },
    /// The patches output mod written from the patched copies of mods' files (`@sat:patch`).
    Patches {
        name: String,
        /// Sources by mod name.
        patches: Vec<crate::patch::FilePatch>,
    },
    /// A factory's output mod built from its group's members, in canon order (`@sat:factory`).
    Bake {
        name: String,
        members: Vec<String>,
        #[serde(default)]
        link: bool,
    },
    /// A planned mod's folder made from its source (`@sat:intent`).
    Install {
        name: String,
        source: crate::intent::Source,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fomod: Option<Vec<String>>,
    },
}

/// Why a plan must not run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "blocker", rename_all = "snake_case")]
pub enum Blocker {
    /// A record waits for a decision.
    Pending { fingerprint: String },
    /// Confirmed without an answer.
    NoResolution { fingerprint: String },
    /// The answer does not fit the mismatch (e.g. "create folder" for a new folder).
    WrongResolution {
        fingerprint: String,
        resolution: Resolution,
    },
    /// A disk operation's source folder is not there.
    MissingFolder { name: String },
    /// A disk operation's target already exists (and is not freed by an earlier operation).
    TargetExists { name: String },
    /// Two operations produce the same name (on disk or in the canon).
    TargetTwice { name: String },
    /// Renames that wait on each other (`a → b`, `b → a`).
    RenameCycle { names: Vec<String> },
    /// A canon change refers to an entry that is not in the canon, or would duplicate one.
    CanonMismatch { name: String },
    /// Taking a separator out of the project while mods are in its block (monolith rule).
    SeparatorNotEmpty { name: String },
    /// The mismatch a decided record describes is no longer what the disk and the canon show (the folder came
    /// back, the entry was edited...): the analysis is stale, run it again (the pre-commit freshness check).
    Stale { fingerprint: String },
    /// A step that is not undone by a later commit (sealing a group): runs only when confirmed by name.
    NeedsConfirmation { what: String, confirm: String },
    /// A patched file changed since the patch was made (the mod was updated): look at the patch again.
    PatchBaseChanged { source: String, path: String },
    /// A planned mod's source is not here or not what the intent saw (`@sat:intent`).
    SourceUnavailable { name: String, why: String },
}

// @sat:sync.plan.build
/// A checked commit plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub project_ops: Vec<ProjectOp>,
    pub disk_ops: Vec<DiskOp>,
    /// The canon after the project operations: what `modlist.txt` and the project receive.
    pub canon_after: Modlist,
    /// Fingerprints of the records this commit settles (closed after a successful commit).
    pub settles: Vec<String>,
    /// Ignored records: they stay, untouched.
    pub ignored: usize,
    pub blockers: Vec<Blocker>,
}

impl Plan {
    pub fn blocked(&self) -> bool {
        !self.blockers.is_empty()
    }
}

fn kind_of(k: EntityKind) -> EntryKind {
    match k {
        EntityKind::Mod => EntryKind::Mod,
        EntityKind::Separator => EntryKind::Separator,
    }
}

/// Builds the plan from the canon, the current records and the folder names on disk now.
pub fn plan(canon: &Modlist, records: &[SyncRecord], disk: &[String]) -> Plan {
    plan_with(canon, records, &[], disk, &|_| Ok(()))
}

// @sat:sync.plan.installs
/// [`plan`] with the pending installs of planned mods (name, source); `source_ok` checks a source on this
/// machine (the file is in the downloads folder, its size is the one the intent saw).
pub fn plan_with(
    canon: &Modlist,
    records: &[SyncRecord],
    installs: &[crate::intent::Planned],
    disk: &[String],
    source_ok: &dyn Fn(&crate::intent::Source) -> Result<(), String>,
) -> Plan {
    let mut p = Plan::default();
    for i in installs {
        match source_ok(&i.source) {
            Ok(()) => p.disk_ops.push(DiskOp::Install {
                name: i.name.clone(),
                source: i.source.clone(),
                fomod: i.fomod.clone(),
            }),
            Err(why) => p.blockers.push(Blocker::SourceUnavailable {
                name: i.name.clone(),
                why,
            }),
        }
    }
    let on_disk: HashSet<String> = disk.iter().map(|d| d.to_lowercase()).collect();
    let in_canon: HashSet<String> = canon.entries.iter().map(|e| e.name.to_lowercase()).collect();
    let disk_has = |n: &str| on_disk.contains(&n.to_lowercase());
    let canon_has = |n: &str| in_canon.contains(&n.to_lowercase());
    let holds = |m: &Mismatch| match m {
        Mismatch::FsCreated { fs_name } => disk_has(fs_name) && !canon_has(fs_name),
        Mismatch::FsDeleted { project_name } => canon_has(project_name) && !disk_has(project_name),
        Mismatch::FsRenamed {
            project_name,
            fs_name,
        } => canon_has(project_name) && disk_has(fs_name) && !disk_has(project_name),
    };
    for r in records {
        match r.status {
            Status::Ignored => {
                p.ignored += 1;
                continue;
            }
            Status::Pending => {
                p.blockers.push(Blocker::Pending {
                    fingerprint: r.fingerprint.clone(),
                });
                continue;
            }
            Status::Confirmed => {}
        }
        let Some(res) = r.resolution else {
            p.blockers.push(Blocker::NoResolution {
                fingerprint: r.fingerprint.clone(),
            });
            continue;
        };
        if !res.fits(&r.mismatch) {
            p.blockers.push(Blocker::WrongResolution {
                fingerprint: r.fingerprint.clone(),
                resolution: res,
            });
            continue;
        }
        if !holds(&r.mismatch) {
            p.blockers.push(Blocker::Stale {
                fingerprint: r.fingerprint.clone(),
            });
            continue;
        }
        p.settles.push(r.fingerprint.clone());
        match (&r.mismatch, res) {
            (Mismatch::FsCreated { fs_name }, Resolution::AdoptToProject) => {
                p.project_ops.push(ProjectOp::Adopt {
                    name: fs_name.clone(),
                    kind: r.entity,
                })
            }
            (Mismatch::FsCreated { fs_name }, _) => p.disk_ops.push(DiskOp::ToDeleted {
                name: fs_name.clone(),
            }),
            (Mismatch::FsDeleted { project_name }, Resolution::CreateDiskFolder) => {
                p.disk_ops.push(DiskOp::Create {
                    name: project_name.clone(),
                    kind: r.entity,
                })
            }
            (Mismatch::FsDeleted { project_name }, _) => p.project_ops.push(ProjectOp::Remove {
                name: project_name.clone(),
            }),
            (
                Mismatch::FsRenamed {
                    project_name,
                    fs_name,
                },
                Resolution::AcceptFsName,
            ) => p.project_ops.push(ProjectOp::Rename {
                from: project_name.clone(),
                to: fs_name.clone(),
            }),
            (
                Mismatch::FsRenamed {
                    project_name,
                    fs_name,
                },
                _,
            ) => p.disk_ops.push(DiskOp::Rename {
                from: fs_name.clone(),
                to: project_name.clone(),
            }),
        }
    }
    p.canon_after = apply_canon(canon, &p.project_ops, &mut p.blockers);
    p.disk_ops = order_disk(std::mem::take(&mut p.disk_ops), disk, &mut p.blockers);
    p
}

// @sat:sync.plan.canon
/// Applies the project operations to a copy of the canon, reporting what does not fit.
fn apply_canon(canon: &Modlist, ops: &[ProjectOp], blockers: &mut Vec<Blocker>) -> Modlist {
    let mut entries = canon.entries.clone();
    let at = |entries: &[Entry], name: &str| entries.iter().position(|e| e.name.eq_ignore_ascii_case(name));
    let mut produced: HashSet<String> = HashSet::new();
    let mut claim = |name: &str, blockers: &mut Vec<Blocker>| {
        if !produced.insert(name.to_lowercase()) {
            blockers.push(Blocker::TargetTwice { name: name.into() });
        }
    };
    for op in ops {
        match op {
            ProjectOp::Remove { name } => match at(&entries, name) {
                None => blockers.push(Blocker::CanonMismatch { name: name.clone() }),
                Some(i) => {
                    let block_has_mods = entries[i].kind == EntryKind::Separator
                        && entries[i + 1..]
                            .iter()
                            .take_while(|e| e.kind != EntryKind::Separator)
                            .any(|e| e.kind == EntryKind::Mod);
                    if block_has_mods {
                        blockers.push(Blocker::SeparatorNotEmpty { name: name.clone() });
                    } else {
                        entries.remove(i);
                    }
                }
            },
            ProjectOp::Rename { from, to } => {
                claim(to, blockers);
                match (at(&entries, from), at(&entries, to)) {
                    (Some(i), None) => entries[i].name = to.clone(),
                    // a change of letter case only is a rename of the same entry
                    (Some(i), Some(j)) if i == j => entries[i].name = to.clone(),
                    _ => blockers.push(Blocker::CanonMismatch { name: to.clone() }),
                }
            }
            ProjectOp::Adopt { name, kind } => {
                claim(name, blockers);
                if at(&entries, name).is_some() {
                    blockers.push(Blocker::CanonMismatch { name: name.clone() });
                } else {
                    entries.push(Entry {
                        name: name.clone(),
                        kind: kind_of(*kind),
                        enabled: *kind == EntityKind::Mod,
                    });
                }
            }
        }
    }
    Modlist::new(entries)
}

// @sat:sync.plan.order
/// Orders disk operations so that every name is free when it is taken, and checks them against the folders now.
fn order_disk(ops: Vec<DiskOp>, disk: &[String], blockers: &mut Vec<Blocker>) -> Vec<DiskOp> {
    let low = |s: &str| s.to_lowercase();
    let mut present: HashSet<String> = disk.iter().map(|d| low(d)).collect();
    let mut out = Vec::with_capacity(ops.len());
    let mut produced: HashMap<String, usize> = HashMap::new();
    for op in &ops {
        let target = match op {
            DiskOp::Rename { to, .. } => Some(to),
            DiskOp::Create { name, .. }
            | DiskOp::Install { name, .. }
            | DiskOp::Bake { name, .. }
            | DiskOp::Patches { name, .. } => Some(name),
            DiskOp::ToDeleted { .. } => None,
        };
        if let Some(t) = target {
            *produced.entry(low(t)).or_default() += 1;
        }
    }
    let mut twice: Vec<&String> = produced.iter().filter(|(_, n)| **n > 1).map(|(k, _)| k).collect();
    twice.sort();
    blockers.extend(
        twice
            .into_iter()
            .map(|name| Blocker::TargetTwice { name: name.clone() }),
    );
    // 1. moves to deleted/ free their names
    for op in &ops {
        if let DiskOp::ToDeleted { name } = op {
            if present.remove(&low(name)) {
                out.push(op.clone());
            } else {
                blockers.push(Blocker::MissingFolder { name: name.clone() });
            }
        }
    }
    // 2. renames: one whose target is still taken by another pending rename's source waits for it
    let mut renames: Vec<(&String, &String)> = ops
        .iter()
        .filter_map(|op| match op {
            DiskOp::Rename { from, to } => Some((from, to)),
            _ => None,
        })
        .collect();
    for (from, _) in &renames {
        if !present.contains(&low(from)) {
            blockers.push(Blocker::MissingFolder {
                name: (*from).clone(),
            });
        }
    }
    renames.retain(|(from, _)| present.contains(&low(from)));
    while !renames.is_empty() {
        let sources: HashSet<String> = renames.iter().map(|(f, _)| low(f)).collect();
        let ready = renames
            .iter()
            .position(|(from, to)| low(from) == low(to) || !sources.contains(&low(to)));
        let Some(i) = ready else {
            blockers.push(Blocker::RenameCycle {
                names: renames.iter().map(|(f, _)| (*f).clone()).collect(),
            });
            break;
        };
        let (from, to) = renames.remove(i);
        if low(from) != low(to) && present.contains(&low(to)) {
            blockers.push(Blocker::TargetExists { name: to.clone() });
            continue;
        }
        present.remove(&low(from));
        present.insert(low(to));
        out.push(DiskOp::Rename {
            from: from.clone(),
            to: to.clone(),
        });
    }
    // 3. creations last
    for op in &ops {
        if let DiskOp::Create { name, .. } | DiskOp::Install { name, .. } = op {
            if present.insert(low(name)) {
                out.push(op.clone());
            } else {
                blockers.push(Blocker::TargetExists { name: name.clone() });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canon(names: &[&str]) -> Modlist {
        Modlist::new(
            names
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: if n.ends_with("_separator") {
                        EntryKind::Separator
                    } else {
                        EntryKind::Mod
                    },
                    enabled: true,
                })
                .collect(),
        )
    }

    fn rec(entity: EntityKind, m: Mismatch, status: Status, res: Option<Resolution>) -> SyncRecord {
        SyncRecord {
            fingerprint: format!("{m:?}"),
            entity,
            mismatch: m,
            status,
            resolution: res,
        }
    }
    fn created(n: &str) -> Mismatch {
        Mismatch::FsCreated { fs_name: n.into() }
    }
    fn deleted(n: &str) -> Mismatch {
        Mismatch::FsDeleted {
            project_name: n.into(),
        }
    }
    fn renamed(p: &str, f: &str) -> Mismatch {
        Mismatch::FsRenamed {
            project_name: p.into(),
            fs_name: f.into(),
        }
    }
    use EntityKind::Mod as M;
    use Status::{Confirmed as C, Ignored as I, Pending as P};
    fn disk(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_answer_becomes_its_operation() {
        let c = canon(&["S_separator", "A", "Gone", "Old", "Lost"]);
        let recs = vec![
            rec(M, created("New"), C, Some(Resolution::AdoptToProject)),
            rec(M, created("Junk"), C, Some(Resolution::MoveToDeleted)),
            rec(M, deleted("Gone"), C, Some(Resolution::RemoveFromProject)),
            rec(M, deleted("Lost"), C, Some(Resolution::CreateDiskFolder)),
            rec(M, renamed("Old", "Old v2"), C, Some(Resolution::AcceptFsName)),
            rec(M, created("Keep"), I, None),
        ];
        let p = plan(
            &c,
            &recs,
            &disk(&["S_separator", "A", "New", "Junk", "Old v2", "Keep"]),
        );
        assert!(!p.blocked(), "{:?}", p.blockers);
        let names: Vec<&str> = p.canon_after.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["S_separator", "A", "Old v2", "Lost", "New"]);
        assert_eq!(
            p.disk_ops,
            [
                DiskOp::ToDeleted { name: "Junk".into() },
                DiskOp::Create {
                    name: "Lost".into(),
                    kind: M
                }
            ]
        );
        assert_eq!((p.settles.len(), p.ignored), (5, 1));
    }

    #[test]
    fn undecided_and_unfitting_answers_block() {
        let c = canon(&["A"]);
        let recs = vec![
            rec(M, created("X"), P, None),
            rec(M, created("Y"), C, None),
            rec(M, created("Z"), C, Some(Resolution::CreateDiskFolder)),
        ];
        let p = plan(&c, &recs, &disk(&["A", "X", "Y", "Z"]));
        assert_eq!(p.blockers.len(), 3);
        assert!(matches!(p.blockers[2], Blocker::WrongResolution { .. }));
    }

    // A reconcile does not produce chains like these (an exact name match wins first), but the order and the
    // collision checks are the last guard before the disk: tested on their own.
    #[test]
    fn renames_free_names_before_taking_them_and_cycles_block() {
        let r = |f: &str, t: &str| DiskOp::Rename {
            from: f.into(),
            to: t.into(),
        };
        let mut b = Vec::new();
        // folder "B" must become "C" before folder "A" can become "B"
        let out = order_disk(vec![r("A", "B"), r("B", "C")], &disk(&["A", "B"]), &mut b);
        assert!(b.is_empty(), "{b:?}");
        assert_eq!(out, [r("B", "C"), r("A", "B")]);
        order_disk(vec![r("A", "B"), r("B", "A")], &disk(&["A", "B"]), &mut b);
        assert!(matches!(b[..], [Blocker::RenameCycle { .. }]));
    }

    #[test]
    fn collisions_are_found_before_anything_runs() {
        let mut b = Vec::new();
        let ops = vec![
            DiskOp::Rename {
                from: "A".into(),
                to: "Taken".into(),
            },
            DiskOp::ToDeleted { name: "Ghost".into() },
            DiskOp::Create {
                name: "Twice".into(),
                kind: M,
            },
            DiskOp::Create {
                name: "Twice".into(),
                kind: M,
            },
        ];
        let out = order_disk(ops, &disk(&["A", "Taken"]), &mut b);
        let kinds: Vec<&str> = b
            .iter()
            .map(|x| match x {
                Blocker::TargetTwice { .. } => "twice",
                Blocker::TargetExists { .. } => "exists",
                Blocker::MissingFolder { .. } => "missing",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["twice", "missing", "exists", "exists"]);
        assert_eq!(out.len(), 1, "only the first creation passed the checks");
        // a separator with mods in its block cannot leave the project
        let recs = vec![rec(
            EntityKind::Separator,
            deleted("S_separator"),
            C,
            Some(Resolution::RemoveFromProject),
        )];
        let p = plan(&canon(&["S_separator", "M"]), &recs, &disk(&["M"]));
        assert!(matches!(p.blockers[..], [Blocker::SeparatorNotEmpty { .. }]));
    }

    #[test]
    fn the_plan_is_a_pure_function() {
        let c = canon(&["A"]);
        let recs = vec![rec(M, created("N"), C, Some(Resolution::AdoptToProject))];
        let d = disk(&["A", "N"]);
        assert_eq!(plan(&c, &recs, &d), plan(&c, &recs, &d));
    }

    #[test]
    fn a_record_the_disk_no_longer_shows_is_stale() {
        // "New" was found by the last analysis, but the folder is gone now
        let recs = vec![rec(M, created("New"), C, Some(Resolution::AdoptToProject))];
        let p = plan(&canon(&["A"]), &recs, &disk(&["A"]));
        assert!(matches!(p.blockers[..], [Blocker::Stale { .. }]));
        assert!(p.canon_after.entries.len() == 1 && p.settles.is_empty());
    }

    #[test]
    fn installs_join_the_plan_and_their_sources_are_checked() {
        use crate::intent::Source;
        let src = |f: &str| Source::Downloads {
            file: f.into(),
            size: None,
        };
        let planned = |n: &str, f: &str| crate::intent::Planned {
            name: n.into(),
            source: src(f),
            fomod: None,
        };
        let c = canon(&["A", "B", "C"]);
        let ok = |s: &Source| match s {
            Source::Downloads { file, .. } if file == "gone.zip" => Err("not in downloads".to_string()),
            _ => Ok(()),
        };
        let p = plan_with(&c, &[], &[planned("B", "b.zip")], &["A".into(), "C".into()], &ok);
        assert!(!p.blocked(), "{:?}", p.blockers);
        assert_eq!(
            p.disk_ops,
            [DiskOp::Install {
                name: "B".into(),
                source: src("b.zip"),
                fomod: None
            }]
        );
        let p = plan_with(&c, &[], &[planned("B", "gone.zip")], &["A".into()], &ok);
        assert!(matches!(&p.blockers[..], [Blocker::SourceUnavailable { name, .. }] if name == "B"));
        // a folder of that name is there already
        let p = plan_with(&c, &[], &[planned("C", "c.zip")], &["A".into(), "C".into()], &ok);
        assert!(matches!(&p.blockers[..], [Blocker::TargetExists { name }] if name == "C"));
    }
}
