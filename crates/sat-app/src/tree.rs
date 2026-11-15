//! The file explorer: trees of mods, of a selection and of a chain, with the conflict overlay. @sat:app.tree
//!
//! Owns: turning a subject — mods by key, the view's selection, a chain of a context — into leaves for
//! `@sat:filetree` (each path with its state relative to the subject, its winner, its chain, whether it is in the
//! focused chain), naming winners and chains for the page, and who owns a path (`PathOwners`): the link from a file
//! of the tree to the conflicts block.
//! Not here: laying out rows (`@sat:filetree`), chains (`@sat:app.chains`), drawing.
//!
//! Monolith (`createTreeModeRenderPayload` v486, `buildTreeConflictOverlayFromGroup` 82 930): the tree of the
//! selection with three modes; a path is won when the subject is its winner, lost when another mod is; the active
//! chain's files are lit (focus layer v486/v488). Several selected mods are one subject: a path is won when its
//! winner is one of them, lost when one of them has it and someone else wins.

use serde::{Deserialize, Serialize};

use sat_core::filetree::{self, Leaf, Mode, State};
use sat_core::ids::{ModId, PathId};
use sat_core::order::{Key, ViewId};

use crate::App;

/// What a tree is of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "of", rename_all = "kebab-case")]
pub enum TreeSubject {
    /// These mods together.
    Mods { keys: Vec<Key> },
    /// The view's selection (the focused entry when nothing is selected).
    Selection { view: ViewId },
    /// Card `index` of the file context of mod `key`: the chain's paths.
    Chain { key: Key, index: u32 },
}

/// A chain a tree refers to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainRef {
    pub signature: String,
    pub emoji: String,
    pub hue: u16,
    pub name: String,
    pub done: bool,
}

/// One row as the page draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRow {
    #[serde(flatten)]
    pub row: filetree::Row,
    /// The winner's name when shared by every conflict file under the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub winner_name: Option<String>,
}

/// The explorer's answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileTree {
    /// Names of the subject's mods (a chain: its stack, winner first).
    pub subject: Vec<String>,
    pub rows: Vec<FileRow>,
    pub hidden: u32,
    pub files: u32,
    pub conflicts: u32,
    /// Chains rows refer to (`row.chain` indexes it).
    pub chains: Vec<ChainRef>,
}

/// Who has a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathOwners {
    pub path: String,
    /// The stack, winner first.
    pub stack: Vec<String>,
    #[serde(default)]
    pub keys: Vec<Option<Key>>,
    /// Disabled owners.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub idle: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainRef>,
}

impl App {
    fn mod_of_key(&self, key: Key) -> Option<ModId> {
        let name = self.session.book().name(key).to_lowercase();
        self.session
            .table()
            .mod_names()
            .iter()
            .position(|n| n.to_lowercase() == name)
            .map(ModId::from_idx)
    }

    // @sat:app.tree.build
    /// The explorer for `subject` in `mode`; `focus` — a chain signature to light; `opened` / `closed` — folders
    /// the user toggled.
    pub(crate) fn file_tree(
        &mut self,
        subject: &TreeSubject,
        mode: Mode,
        focus: Option<&str>,
        opened: &[String],
        closed: &[String],
        limit: usize,
    ) -> Option<FileTree> {
        let cx = match subject {
            TreeSubject::Chain { key, .. } => Some(self.context_of(*key)?),
            _ => None,
        };
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let heads = self.heads();
        let focus_head = focus
            .and_then(|f| heads.iter().position(|h| h.hex() == f))
            .map(|i| i as u32);
        let head_of = |p: PathId| ow.path_chain(p).and_then(|c| heads.of_chain(c));
        let label = |m: ModId| self.session.unit_label(m).to_string();
        let mut winners: Vec<ModId> = Vec::new();
        let mut win_id = |m: ModId| match winners.iter().position(|&x| x == m) {
            Some(i) => i as u32,
            None => {
                winners.push(m);
                (winners.len() - 1) as u32
            }
        };
        let mut leaves: Vec<Leaf> = Vec::new();
        let subject_names: Vec<String>;
        match subject {
            TreeSubject::Chain { index, .. } => {
                let cx = cx?;
                let book = self.session.book();
                let pos_of = |m: ModId| book.key_of(table.mod_name(m)).map_or(usize::MAX, |k| book.pos(k));
                let g = *crate::context::context_groups(&ow, &heads, cx, &pos_of).get(*index as usize)?;
                let h = &heads[g as usize];
                subject_names = h.route.iter().map(|&m| label(m)).collect();
                for &c in &h.chains {
                    let w = win_id(ow.chain_mods(c)[0]);
                    for p in ow.chain_paths(c) {
                        leaves.push(Leaf {
                            path: table.path_string(p),
                            state: State::Contested,
                            winner: w,
                            chain: g,
                            focus: focus_head == Some(g),
                        });
                    }
                }
            }
            TreeSubject::Mods { .. } | TreeSubject::Selection { .. } => {
                let keys: Vec<Key> = match subject {
                    TreeSubject::Mods { keys } => keys.clone(),
                    TreeSubject::Selection { view } => {
                        let book = self.session.book();
                        book.selection(*view)
                            .into_iter()
                            .filter(|&k| book.kind(k) == sat_core::canon::EntryKind::Mod)
                            .collect()
                    }
                    TreeSubject::Chain { .. } => unreachable!(),
                };
                let raw: Vec<ModId> = keys.iter().filter_map(|&k| self.mod_of_key(k)).collect();
                if raw.is_empty() {
                    return None;
                }
                subject_names = keys
                    .iter()
                    .map(|&k| self.session.book().name(k).to_string())
                    .collect();
                let units: Vec<ModId> = raw.iter().map(|&m| ow.units().rep(m)).collect();
                let mut paths: Vec<PathId> = raw
                    .iter()
                    .flat_map(|&m| {
                        table
                            .mod_providers(m)
                            .iter()
                            .map(|&i| table.path_of_provider(i as usize))
                    })
                    .collect();
                paths.sort_unstable();
                paths.dedup();
                for p in paths {
                    let (state, winner, chain) = match ow.path_chain(p) {
                        Some(c) => {
                            let st = ow.chain_mods(c);
                            let state = if units.contains(&st[0]) {
                                State::Win
                            } else if st.iter().any(|m| units.contains(m)) {
                                State::Lose
                            } else {
                                State::Owned
                            };
                            (state, win_id(st[0]), head_of(p).unwrap_or(filetree::NONE))
                        }
                        None => (State::Plain, filetree::NONE, filetree::NONE),
                    };
                    leaves.push(Leaf {
                        path: table.path_string(p),
                        state,
                        winner,
                        chain,
                        focus: focus_head.is_some() && chain == focus_head.unwrap_or(filetree::NONE),
                    });
                }
            }
        }
        let opener = |p: &str| {
            let l = p.to_lowercase();
            if closed.iter().any(|c| c.to_lowercase() == l) {
                Some(false)
            } else if opened.iter().any(|c| c.to_lowercase() == l) {
                Some(true)
            } else {
                None
            }
        };
        let t = filetree::build(&leaves, mode, &opener, limit);
        // chains rows refer to, renumbered for the page
        let mut chain_ids: Vec<u32> = Vec::new();
        let decisions = self.session.decisions();
        let mut rows: Vec<FileRow> = Vec::with_capacity(t.rows.len());
        for mut r in t.rows {
            if r.chain != filetree::NONE {
                let g = r.chain;
                r.chain = match chain_ids.iter().position(|&x| x == g) {
                    Some(i) => i as u32,
                    None => {
                        chain_ids.push(g);
                        (chain_ids.len() - 1) as u32
                    }
                };
            }
            let winner_name = (r.winner != filetree::NONE).then(|| label(winners[r.winner as usize]));
            rows.push(FileRow { row: r, winner_name });
        }
        Some(FileTree {
            subject: subject_names,
            rows,
            hidden: t.hidden,
            files: t.files,
            conflicts: t.conflicts,
            chains: chain_ids
                .iter()
                .map(|&g| {
                    let h = &heads[g as usize];
                    ChainRef {
                        signature: h.hex(),
                        emoji: h.emoji.clone(),
                        hue: h.hue,
                        name: h.name.clone(),
                        done: h.done(decisions),
                    }
                })
                .collect(),
        })
    }

    // @sat:app.tree.owners
    /// Who has `path`: the stack and the disabled owners, and its chain.
    pub(crate) fn path_owners(&mut self, path: &str) -> Option<PathOwners> {
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let p = table.find_path(path)?;
        let heads = self.heads();
        let book = self.session.book();
        let (stack, chain) = match ow.path_chain(p) {
            Some(c) => (ow.chain_mods(c).to_vec(), heads.of_chain(c)),
            None => {
                let mut v: Vec<ModId> = table
                    .path_providers(p)
                    .map(|i| ow.units().rep(table.owner(i)))
                    .filter(|&m| ow.priorities().is_active(m))
                    .collect();
                v.dedup();
                (v, None)
            }
        };
        let decisions = self.session.decisions();
        Some(PathOwners {
            path: table.path_string(p),
            keys: stack.iter().map(|&m| book.key_of(table.mod_name(m))).collect(),
            stack: stack
                .iter()
                .map(|&m| self.session.unit_label(m).to_string())
                .collect(),
            idle: ow
                .idle_owners(&table, p)
                .into_iter()
                .map(|m| self.session.unit_label(m).to_string())
                .collect(),
            chain: chain.map(|g| {
                let h = &heads[g as usize];
                ChainRef {
                    signature: h.hex(),
                    emoji: h.emoji.clone(),
                    hue: h.hue,
                    name: h.name.clone(),
                    done: h.done(decisions),
                }
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use sat_core::filetree::{Kind, Mode, State};

    use super::TreeSubject;
    use crate::{Query, Reply};

    fn tree(a: &mut crate::App, subject: TreeSubject, mode: Mode, focus: Option<String>) -> super::FileTree {
        let Reply::FileTree { tree: Some(t) } = a
            .query(Query::FileTree {
                subject,
                mode,
                focus,
                opened: vec![],
                closed: vec![],
                limit: 500,
            })
            .unwrap()
        else {
            panic!()
        };
        t
    }

    #[test]
    fn a_mods_tree_marks_what_it_wins_and_loses_and_names_the_winner() {
        let mut a = crate::tests::app_of(["A", "B"]);
        let ka = a.session().book().key_of("A").unwrap();
        let t = tree(&mut a, TreeSubject::Mods { keys: vec![ka] }, Mode::Classic, None);
        let f = t.rows.iter().find(|r| r.row.kind == Kind::File).unwrap();
        assert_eq!((f.row.name.as_str(), f.row.state), ("a.dds", State::Lose));
        assert_eq!(f.winner_name.as_deref(), Some("B"));
        assert_eq!(t.rows[0].row.loses, 1, "the folder sums it");
        assert_eq!(t.chains.len(), 1);
        // both mods: the set wins
        let kb = a.session().book().key_of("B").unwrap();
        let t = tree(
            &mut a,
            TreeSubject::Mods { keys: vec![ka, kb] },
            Mode::Conflict,
            None,
        );
        assert_eq!(
            t.rows
                .iter()
                .find(|r| r.row.kind == Kind::File)
                .unwrap()
                .row
                .state,
            State::Win
        );
        // the focused chain lights its files
        let sig = t.chains[0].signature.clone();
        let t = tree(
            &mut a,
            TreeSubject::Mods { keys: vec![ka] },
            Mode::Grouped,
            Some(sig),
        );
        assert_eq!(t.rows[0].row.focused, 1);
    }

    #[test]
    fn a_chains_tree_holds_its_paths_and_owners_name_the_stack() {
        let mut a = crate::tests::app_of(["A", "B"]);
        let ka = a.session().book().key_of("A").unwrap();
        let t = tree(
            &mut a,
            TreeSubject::Chain { key: ka, index: 0 },
            Mode::Conflict,
            None,
        );
        assert_eq!(t.subject, ["B", "A"]);
        assert_eq!(t.files, 1);
        let Reply::PathOwners { owners: Some(o) } = a
            .query(Query::PathOwners {
                path: "Textures/A.dds".into(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(o.stack, ["B", "A"]);
        assert!(o.chain.is_some());
    }
}
