//! [`OrderBook`] — the live canon of a list (any size) and its visible projection, changed only by operations
//! that return **row patches**. A UI never re-renders the list: it keeps the row count and its visible window
//! and applies [`Patch`]es (ADR 0008).
//!
//! Three layers, never mixed:
//! - **canon** — order and enabled flags (what goes to `modlist.txt`);
//! - **decoration** — colour, logical groups: stored next to the canon, never reorder it;
//! - **view** — collapsed separators and groups, filter: decide which entries are rows.

use std::collections::HashSet;

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use super::cursor::Cursor;
use super::decor::{Decor, DecorIssue, GroupDef};
use super::seq::{Key, Seq};
use crate::canon::{Entry, EntryKind, Modlist};

pub type GroupId = u32;

/// A change of the visible rows. Patches of one [`Change`] apply in order; each index refers to the rows as
/// they are after the previous patch. A UI fetches row contents with [`OrderBook::rows`] when it needs them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Patch {
    /// Rows `[at, at + count)` disappear.
    Remove { at: u32, count: u32 },
    /// `count` new rows appear at `at`.
    Insert { at: u32, count: u32 },
    /// Rows `[at, at + count)` keep their place but changed content.
    Update { at: u32, count: u32 },
    /// Everything changed (a new filter): the list now has `len` rows.
    Reset { len: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Revision of the book after the change.
    pub revision: u64,
    /// Row count after the change.
    pub len: u32,
    pub patches: Vec<Patch>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrderError {
    #[error("unknown key {0}")]
    UnknownKey(Key),
    #[error("entry {0:?} cannot be moved")]
    Immovable(String),
    #[error("an entry named {0:?} already exists")]
    Duplicate(String),
    #[error("nothing to insert")]
    Empty,
    #[error("cannot group: {0}")]
    Group(&'static str),
}

/// What a row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RowKind {
    Mod,
    Separator,
    Foreign,
    /// A collapsed logical group shown as one row (anchored at its first member).
    Group,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row<'a> {
    pub key: Key,
    /// Canon position (0-based, display order; MO2 priority). **Not covered by patches**: one move shifts the
    /// position of every entry between source and target, so a UI re-reads its window after each change.
    pub pos: u32,
    pub kind: RowKind,
    pub name: &'a str,
    /// For a group row: every member enabled.
    pub enabled: bool,
    pub depth: u8,
    pub collapsed: bool,
    pub color: Option<u32>,
    /// Separator: entries in its block. Group: members.
    pub size: u32,
    pub group: Option<GroupId>,
    /// Hidden from views unless the filter shows muted entries ("not planned, keep out of sight").
    pub muted: bool,
    /// The keyboard cursor is on this row.
    pub focused: bool,
    pub selected: bool,
}

/// Which mods are rows. Separators always are.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    /// Case-insensitive substring of the name.
    pub text: String,
    pub enabled: Option<bool>,
    pub color: Option<u32>,
    /// Show entries the user muted (hidden by default).
    #[serde(default)]
    pub show_muted: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Item {
    pub(super) name: String,
    pub(super) folded: String,
    pub(super) kind: EntryKind,
    pub(super) enabled: bool,
    pub(super) collapsed: bool,
    pub(super) color: Option<u32>,
    pub(super) group: Option<GroupId>,
    pub(super) muted: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Group {
    name: String,
    pub(super) members: Vec<Key>,
    pub(super) collapsed: bool,
    /// Members are contiguous (hence in one block). A broken group shows its members as plain rows.
    pub(super) valid: bool,
    pub(super) first: Key,
}

#[derive(Debug, Clone)]
enum Op {
    Move {
        keys: Vec<Key>,
        before: Option<Key>,
    },
    Place(Vec<(Key, usize)>),
    Enabled(Vec<(Key, bool)>),
    Color(Vec<(Key, Option<u32>)>),
    Muted(Vec<(Key, bool)>),
    MakeGroup {
        id: GroupId,
        name: String,
        members: Vec<Key>,
        collapsed: bool,
    },
    Dissolve(GroupId),
    /// Puts entries that are not in the list (new or taken) before `before`.
    Put {
        keys: Vec<Key>,
        before: Option<Key>,
    },
    /// Takes entries out of the list.
    Take(Vec<Key>),
    /// Puts taken entries back at their positions.
    Restore(Vec<(Key, usize)>),
    Rename(Key, String),
}

#[derive(Default)]
pub(super) struct Patches(pub(super) Vec<Patch>);

impl Patches {
    pub(super) fn remove(&mut self, at: usize) {
        let at = at as u32;
        if let Some(Patch::Remove { at: a, count }) = self.0.last_mut() {
            if *a == at {
                *count += 1;
                return;
            }
            if at + 1 == *a {
                *a = at;
                *count += 1;
                return;
            }
        }
        self.0.push(Patch::Remove { at, count: 1 });
    }
    pub(super) fn insert(&mut self, at: usize) {
        let at = at as u32;
        if let Some(Patch::Insert { at: a, count }) = self.0.last_mut()
            && at == *a + *count
        {
            *count += 1;
            return;
        }
        self.0.push(Patch::Insert { at, count: 1 });
    }
    pub(super) fn update(&mut self, at: usize) {
        let at = at as u32;
        if let Some(Patch::Update { at: a, count }) = self.0.last_mut() {
            if at >= *a && at < *a + *count {
                return;
            }
            if at == *a + *count {
                *count += 1;
                return;
            }
        }
        self.0.push(Patch::Update { at, count: 1 });
    }
}

/// The live canon and its rows.
#[derive(Debug, Clone)]
pub struct OrderBook {
    pub(super) items: Vec<Item>,
    by_name: FxHashMap<String, Key>,
    pub(super) seq: Seq,
    pub(super) groups: Vec<Option<Group>>,
    /// Stable external id of each group slot (marker token, project file).
    group_ids: Vec<String>,
    filter: Filter,
    revision: u64,
    undo: Vec<(Op, Op)>,
    redo: Vec<(Op, Op)>,
    pub(super) cursor: Cursor,
}

impl OrderBook {
    /// Builds the book from a canon list. Key `i` is entry `i` of the list.
    pub fn from_modlist(list: &Modlist) -> Self {
        let items: Vec<Item> = list
            .entries
            .iter()
            .map(|e| Item {
                name: e.name.clone(),
                folded: e.name.to_lowercase(),
                kind: e.kind,
                enabled: e.enabled,
                collapsed: false,
                color: None,
                group: None,
                muted: false,
            })
            .collect();
        let by_name = items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.folded.clone(), i as Key))
            .collect();
        let keys: Vec<Key> = (0..items.len() as Key).collect();
        let seq = Seq::from_keys(&keys, |k| (true, items[k as usize].kind == EntryKind::Separator));
        Self {
            items,
            by_name,
            seq,
            groups: Vec::new(),
            group_ids: Vec::new(),
            filter: Filter::default(),
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
            cursor: Cursor::default(),
        }
    }

    /// The canon in display order.
    pub fn to_modlist(&self) -> Modlist {
        Modlist::new(
            self.seq
                .iter_from(0)
                .map(|k| {
                    let it = &self.items[k as usize];
                    Entry {
                        name: it.name.clone(),
                        kind: it.kind,
                        enabled: it.enabled,
                    }
                })
                .collect(),
        )
    }

    pub fn len(&self) -> usize {
        self.seq.len()
    }
    pub fn is_empty(&self) -> bool {
        self.seq.is_empty()
    }
    pub fn row_count(&self) -> usize {
        self.seq.visible_len()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn key_of(&self, name: &str) -> Option<Key> {
        self.by_name.get(&name.to_lowercase()).copied()
    }
    pub fn pos(&self, k: Key) -> usize {
        self.seq.pos(k)
    }
    pub fn key_at(&self, pos: usize) -> Key {
        self.seq.at(pos)
    }
    pub fn kind(&self, k: Key) -> EntryKind {
        self.items[k as usize].kind
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub(super) fn check_key(&self, k: Key) -> Result<(), OrderError> {
        if self.seq.contains(k) {
            Ok(())
        } else {
            Err(OrderError::UnknownKey(k))
        }
    }

    pub(super) fn group(&self, k: Key) -> Option<(GroupId, &Group)> {
        let g = self.items[k as usize].group?;
        self.groups[g as usize].as_ref().map(|gr| (g, gr))
    }

    fn passes_filter(&self, it: &Item) -> bool {
        let f = &self.filter;
        (f.text.is_empty() || it.folded.contains(&f.text))
            && f.enabled.is_none_or(|e| e == it.enabled)
            && f.color.is_none_or(|c| it.color == Some(c))
            && (f.show_muted || !it.muted)
    }

    /// Whether entry `k` is a row now, given `collapsed` = the state of its block's separator.
    fn want_with(&self, k: Key, block_collapsed: bool) -> bool {
        let it = &self.items[k as usize];
        if it.kind == EntryKind::Separator {
            return true;
        }
        if block_collapsed || !self.passes_filter(it) {
            return false;
        }
        match self.group(k) {
            Some((_, g)) => !(g.valid && g.collapsed && g.first != k),
            None => true,
        }
    }

    fn want(&self, k: Key) -> bool {
        let collapsed = self.items[k as usize].kind != EntryKind::Separator
            && self
                .seq
                .separator_before(self.seq.pos(k))
                .is_some_and(|s| self.items[s as usize].collapsed);
        self.want_with(k, collapsed)
    }

    /// Re-evaluates rows of `keys` (any order; sorted here), emitting patches; `update(k)` marks a row whose
    /// content changed in place.
    pub(super) fn reweigh(
        &mut self,
        keys: impl IntoIterator<Item = Key>,
        update: impl Fn(Key) -> bool,
        p: &mut Patches,
    ) {
        let mut ks: Vec<(usize, Key)> = keys.into_iter().map(|k| (self.seq.pos(k), k)).collect();
        ks.sort_unstable();
        ks.dedup();
        for (_, k) in ks {
            let (had, want) = (self.seq.is_visible(k), self.want(k));
            match (had, want) {
                (false, true) => {
                    self.seq.set_visible(k, true);
                    p.insert(self.seq.rows_before(k));
                }
                (true, false) => {
                    p.remove(self.seq.rows_before(k));
                    self.seq.set_visible(k, false);
                }
                (true, true) if update(k) => p.update(self.seq.rows_before(k)),
                _ => {}
            }
        }
    }

    /// Entries of the block after separator `s` (excluding `s`).
    pub(super) fn block_keys(&self, s: Key) -> Vec<Key> {
        let start = self.seq.pos(s) + 1;
        let end = self.seq.block_end(start);
        self.seq.iter_from(start).take(end - start).collect()
    }

    pub(super) fn owner(&self, k: Key) -> Option<Key> {
        if self.items[k as usize].kind == EntryKind::Separator {
            return None;
        }
        self.seq.separator_before(self.seq.pos(k))
    }

    /// Recomputes validity and first member of a group; returns `true` when the presentation changed.
    fn recheck_group(&mut self, g: GroupId) -> bool {
        let Some(gr) = self.groups[g as usize].as_ref() else {
            return false;
        };
        let pos: Vec<(usize, Key)> = gr.members.iter().map(|&k| (self.seq.pos(k), k)).collect();
        let (lo, first) = *pos.iter().min().unwrap();
        let hi = pos.iter().map(|p| p.0).max().unwrap();
        let valid = hi - lo + 1 == pos.len();
        let gr = self.groups[g as usize].as_mut().unwrap();
        let changed = gr.valid != valid || gr.first != first;
        gr.valid = valid;
        gr.first = first;
        changed
    }

    pub(super) fn finish(&mut self, mut p: Patches) -> Change {
        self.sync_focus_mark(&mut p);
        self.seq.compact();
        self.revision += 1;
        Change {
            revision: self.revision,
            len: self.seq.visible_len() as u32,
            patches: p.0,
        }
    }

    fn record(&mut self, fwd: Op, inv: Op) {
        self.undo.push((fwd, inv));
        self.redo.clear();
    }

    // ---------------------------------------------------------------- canon operations (undoable)

    /// Moves entries right before `before` (`None` = end), keeping their relative order. A separator moves with
    /// its block; a collapsed group row moves with its members. A target inside a group snaps to the group's
    /// start unless the moved entries include its members. `Ok(None)` = nothing changed.
    pub fn move_keys(&mut self, keys: &[Key], before: Option<Key>) -> Result<Option<Change>, OrderError> {
        let fwd = Op::Move {
            keys: keys.to_vec(),
            before,
        };
        let mut p = Patches::default();
        let Some(inv) = self.apply(&fwd, &mut p)? else {
            return Ok(None);
        };
        self.record(fwd, inv);
        Ok(Some(self.finish(p)))
    }

    /// Sets (or toggles with `None`) the enabled flag of mods; other entries are ignored.
    pub fn set_enabled(&mut self, keys: &[Key], enabled: Option<bool>) -> Result<Option<Change>, OrderError> {
        let mut each = Vec::new();
        for &k in keys {
            self.check_key(k)?;
            let it = &self.items[k as usize];
            if it.kind == EntryKind::Mod {
                each.push((k, enabled.unwrap_or(!it.enabled)));
            }
        }
        self.apply_recorded(Op::Enabled(each))
    }

    /// Sets the decoration colour (`None` clears it).
    pub fn set_color(&mut self, keys: &[Key], color: Option<u32>) -> Result<Option<Change>, OrderError> {
        for &k in keys {
            self.check_key(k)?;
        }
        self.apply_recorded(Op::Color(keys.iter().map(|&k| (k, color)).collect()))
    }

    /// Mutes (hides unless the filter shows muted entries) or unmutes entries. Separators cannot be muted.
    pub fn set_muted(&mut self, keys: &[Key], muted: bool) -> Result<Option<Change>, OrderError> {
        for &k in keys {
            self.check_key(k)?;
        }
        self.apply_recorded(Op::Muted(keys.iter().map(|&k| (k, muted)).collect()))
    }

    /// Joins contiguous mods of one block into a logical group (shown collapsed as one row).
    pub fn create_group(&mut self, name: &str, keys: &[Key]) -> Result<(GroupId, Change), OrderError> {
        for &k in keys {
            self.check_key(k)?;
        }
        let members = self.group_rule(keys, true).map_err(OrderError::Group)?;
        let id = self.groups.len() as GroupId;
        self.group_ids.push(format!("lg-{id}"));
        let fwd = Op::MakeGroup {
            id,
            name: name.to_string(),
            members,
            collapsed: true,
        };
        let change = self.apply_recorded(fwd)?.expect("group creation changes rows");
        Ok((id, change))
    }

    /// Monolith rule (44353): at least two mods, contiguous (for a new group), not in another group. Returns members in canon order.
    fn group_rule(&self, keys: &[Key], contiguous: bool) -> Result<Vec<Key>, &'static str> {
        let mut members: Vec<Key> = keys.to_vec();
        members.sort_unstable();
        members.dedup();
        if members.len() < 2 {
            return Err("need at least two mods");
        }
        for &k in &members {
            let it = &self.items[k as usize];
            if it.kind != EntryKind::Mod {
                return Err("only mods can be grouped");
            }
            if it.group.is_some() {
                return Err("contains members of another group");
            }
        }
        members.sort_by_key(|&k| self.seq.pos(k));
        let (lo, hi) = (self.seq.pos(members[0]), self.seq.pos(members[members.len() - 1]));
        if contiguous && hi - lo + 1 != members.len() {
            return Err("selection is not contiguous");
        }
        Ok(members)
    }

    /// Removes a logical group; its members stay where they are.
    pub fn dissolve_group(&mut self, g: GroupId) -> Result<Option<Change>, OrderError> {
        if self.groups.get(g as usize).is_none_or(Option::is_none) {
            return Err(OrderError::Group("no such group"));
        }
        self.apply_recorded(Op::Dissolve(g))
    }

    /// Adds new entries right before `before` (`None` = end), e.g. folders found by SYNC. Names must be new.
    pub fn insert_entries(
        &mut self,
        entries: &[Entry],
        before: Option<Key>,
    ) -> Result<(Vec<Key>, Change), OrderError> {
        if let Some(b) = before {
            self.check_key(b)?;
        }
        if entries.is_empty() {
            return Err(OrderError::Empty);
        }
        let mut seen = HashSet::new();
        for e in entries {
            let f = e.name.to_lowercase();
            if self.by_name.contains_key(&f) || !seen.insert(f) {
                return Err(OrderError::Duplicate(e.name.clone()));
            }
        }
        let keys: Vec<Key> = entries
            .iter()
            .map(|e| {
                self.items.push(Item {
                    name: e.name.clone(),
                    folded: e.name.to_lowercase(),
                    kind: e.kind,
                    enabled: e.enabled,
                    collapsed: false,
                    color: None,
                    group: None,
                    muted: false,
                });
                (self.items.len() - 1) as Key
            })
            .collect();
        let fwd = Op::Put {
            keys: keys.clone(),
            before,
        };
        let change = self.apply_recorded(fwd)?.expect("insertion changes the list");
        Ok((keys, change))
    }

    /// Takes entries out of the list (undoable). A separator goes alone: its block joins the block above.
    /// Members of a logical group must leave the group first.
    pub fn remove_entries(&mut self, keys: &[Key]) -> Result<Option<Change>, OrderError> {
        let mut uniq: Vec<Key> = Vec::new();
        let mut seen = HashSet::new();
        for &k in keys {
            self.check_key(k)?;
            if self.items[k as usize].group.is_some() {
                return Err(OrderError::Group(
                    "dissolve the group before removing its members",
                ));
            }
            if seen.insert(k) {
                uniq.push(k);
            }
        }
        if uniq.is_empty() {
            return Ok(None);
        }
        self.apply_recorded(Op::Take(uniq))
    }

    /// Renames an entry (e.g. accepting a folder renamed on disk). The new name must be free; a separator keeps
    /// its `_separator` suffix only if the caller passes it.
    pub fn rename(&mut self, k: Key, name: &str) -> Result<Option<Change>, OrderError> {
        self.check_key(k)?;
        if let Some(&other) = self.by_name.get(&name.to_lowercase())
            && other != k
        {
            return Err(OrderError::Duplicate(name.to_string()));
        }
        self.apply_recorded(Op::Rename(k, name.to_string()))
    }

    fn name_keys(&mut self, keys: &[Key], present: bool) {
        for &k in keys {
            let f = self.items[k as usize].folded.clone();
            if present {
                self.by_name.insert(f, k);
            } else {
                self.by_name.remove(&f);
            }
        }
    }

    fn apply_recorded(&mut self, fwd: Op) -> Result<Option<Change>, OrderError> {
        let mut p = Patches::default();
        let Some(inv) = self.apply(&fwd, &mut p)? else {
            return Ok(None);
        };
        self.record(fwd, inv);
        Ok(Some(self.finish(p)))
    }

    pub fn undo(&mut self) -> Option<Change> {
        let (fwd, inv) = self.undo.pop()?;
        let mut p = Patches::default();
        self.apply(&inv, &mut p).expect("inverse operations are valid");
        self.redo.push((fwd, inv));
        Some(self.finish(p))
    }

    pub fn redo(&mut self) -> Option<Change> {
        let (fwd, inv) = self.redo.pop()?;
        let mut p = Patches::default();
        self.apply(&fwd, &mut p).expect("redone operations are valid");
        self.undo.push((fwd, inv));
        Some(self.finish(p))
    }

    /// Applies an operation; returns its inverse, or `None` when nothing changed.
    fn apply(&mut self, op: &Op, p: &mut Patches) -> Result<Option<Op>, OrderError> {
        Ok(match op {
            Op::Move { keys, before } => self.apply_move(keys, *before, p)?,
            Op::Place(items) => {
                let mut keys: Vec<Key> = items.iter().map(|x| x.0).collect();
                keys.sort_by_key(|&k| self.seq.pos(k));
                let inv: Vec<(Key, usize)> = keys.iter().map(|&k| (k, self.seq.pos(k))).collect();
                self.relocate(&keys, Placement::At(items), p);
                Some(Op::Place(inv))
            }
            Op::Enabled(each) => {
                let mut inv = Vec::new();
                let mut touched = HashSet::new();
                for &(k, v) in each {
                    let it = &mut self.items[k as usize];
                    if it.enabled != v {
                        inv.push((k, it.enabled));
                        it.enabled = v;
                        touched.insert(k);
                    }
                }
                self.after_content_change(touched, p);
                (!inv.is_empty()).then_some(Op::Enabled(inv))
            }
            Op::Color(each) => {
                let mut inv = Vec::new();
                let mut touched = HashSet::new();
                for &(k, c) in each {
                    let it = &mut self.items[k as usize];
                    if it.color != c {
                        inv.push((k, it.color));
                        it.color = c;
                        touched.insert(k);
                    }
                }
                self.after_content_change(touched, p);
                (!inv.is_empty()).then_some(Op::Color(inv))
            }
            Op::Muted(each) => {
                let mut inv = Vec::new();
                let mut touched = HashSet::new();
                for &(k, m) in each {
                    let it = &mut self.items[k as usize];
                    if it.muted != m && it.kind != EntryKind::Separator {
                        inv.push((k, it.muted));
                        it.muted = m;
                        touched.insert(k);
                    }
                }
                self.after_content_change(touched, p);
                (!inv.is_empty()).then_some(Op::Muted(inv))
            }
            Op::MakeGroup {
                id,
                name,
                members,
                collapsed,
            } => {
                let id = *id;
                if self.groups.len() <= id as usize {
                    self.groups.resize(id as usize + 1, None);
                }
                self.groups[id as usize] = Some(Group {
                    name: name.clone(),
                    members: members.clone(),
                    collapsed: *collapsed,
                    valid: false,
                    first: members[0],
                });
                for &k in members {
                    self.items[k as usize].group = Some(id);
                }
                self.recheck_group(id);
                // members change depth, the first one becomes the group row
                self.reweigh(members.clone(), |_| true, p);
                Some(Op::Dissolve(id))
            }
            Op::Put { keys, before } => {
                self.relocate(keys, Placement::Before(*before), p);
                self.name_keys(keys, true);
                Some(Op::Take(keys.clone()))
            }
            Op::Restore(items) => {
                let keys: Vec<Key> = items.iter().map(|x| x.0).collect();
                self.relocate(&keys, Placement::At(items), p);
                self.name_keys(&keys, true);
                Some(Op::Take(keys))
            }
            Op::Take(keys) => {
                let mut keys = keys.clone();
                keys.sort_by_key(|&k| self.seq.pos(k));
                let inv: Vec<(Key, usize)> = keys.iter().map(|&k| (k, self.seq.pos(k))).collect();
                self.relocate(&keys, Placement::Nowhere, p);
                self.name_keys(&keys, false);
                self.cursor_forget(&keys, inv.first().map_or(0, |x| x.1), p);
                Some(Op::Restore(inv))
            }
            Op::Rename(k, name) => {
                let it = &mut self.items[*k as usize];
                if it.name == *name {
                    return Ok(None);
                }
                let old = std::mem::replace(&mut it.name, name.clone());
                let (old_f, new_f) = (
                    std::mem::replace(&mut it.folded, name.to_lowercase()),
                    name.to_lowercase(),
                );
                self.by_name.remove(&old_f);
                self.by_name.insert(new_f, *k);
                self.after_content_change(HashSet::from([*k]), p);
                Some(Op::Rename(*k, old))
            }
            Op::Dissolve(id) => {
                let g = self.groups[*id as usize].take().expect("group exists");
                for &k in &g.members {
                    self.items[k as usize].group = None;
                }
                self.reweigh(g.members.clone(), |_| true, p);
                Some(Op::MakeGroup {
                    id: *id,
                    name: g.name,
                    members: g.members,
                    collapsed: g.collapsed,
                })
            }
        })
    }

    /// Rows whose content depends on changed entries: the entries themselves and their groups' rows.
    pub(super) fn after_content_change(&mut self, touched: HashSet<Key>, p: &mut Patches) {
        let mut keys: HashSet<Key> = touched.clone();
        for &k in &touched {
            if let Some((_, g)) = self.group(k) {
                keys.insert(g.first);
            }
        }
        self.reweigh(keys, |_| true, p);
    }

    fn apply_move(
        &mut self,
        keys: &[Key],
        before: Option<Key>,
        p: &mut Patches,
    ) -> Result<Option<Op>, OrderError> {
        let mut picked: HashSet<Key> = HashSet::new();
        for &k in keys {
            self.check_key(k)?;
            let it = &self.items[k as usize];
            match it.kind {
                EntryKind::Foreign => return Err(OrderError::Immovable(it.name.clone())),
                EntryKind::Separator => {
                    picked.insert(k);
                    picked.extend(self.block_keys(k));
                }
                EntryKind::Mod => {
                    picked.insert(k);
                    if let Some((_, g)) = self.group(k)
                        && g.valid
                        && g.collapsed
                        && g.first == k
                    {
                        picked.extend(g.members.iter().copied());
                    }
                }
            }
        }
        let mut before = before;
        if let Some(b) = before {
            self.check_key(b)?;
            if picked.contains(&b) {
                return Ok(None);
            }
            if let Some((_, g)) = self.group(b) {
                // an outsider dropped inside a group lands before it; members may reorder inside
                if g.valid && b != g.first && !g.members.iter().any(|m| picked.contains(m)) {
                    before = Some(g.first);
                }
            }
        }
        let mut moved: Vec<(usize, Key)> = picked.iter().map(|&k| (self.seq.pos(k), k)).collect();
        moved.sort_unstable();
        let (lo, hi) = (moved[0].0, moved[moved.len() - 1].0);
        let target = before.map_or(self.seq.len(), |b| self.seq.pos(b));
        if hi - lo + 1 == moved.len() && target == hi + 1 {
            return Ok(None);
        }
        let inv = Op::Place(moved.iter().map(|&(pos, k)| (k, pos)).collect());
        let keys: Vec<Key> = moved.into_iter().map(|x| x.1).collect();
        self.relocate(&keys, Placement::Before(before), p);
        Ok(Some(inv))
    }

    /// Takes `keys` (in canon order) out and puts them back per `placement`, emitting row patches for every
    /// entry whose row changed: the moved entries, entries whose block changed, group rows, separator headers.
    fn relocate(&mut self, keys: &[Key], placement: Placement<'_>, p: &mut Patches) {
        let set: HashSet<Key> = keys.iter().copied().collect();
        let mut touched: HashSet<Key> = HashSet::new();
        let mut headers: HashSet<Key> = HashSet::new();
        let mut neighbours: Vec<Key> = Vec::new();
        let len = self.seq.len();
        for &k in keys.iter().filter(|&&k| self.seq.contains(k)) {
            if self.items[k as usize].kind == EntryKind::Separator {
                touched.extend(self.block_keys(k).into_iter().filter(|x| !set.contains(x)));
            }
            let pos = self.seq.pos(k);
            // the block the entry is in (for a separator: the block it cuts) changes size
            if let Some(s) = self.seq.separator_before(pos) {
                headers.insert(s);
            }
            if pos > 0 {
                neighbours.push(self.seq.at(pos - 1));
            }
            if pos + 1 < len {
                neighbours.push(self.seq.at(pos + 1));
            }
        }
        // Out, from the back so earlier row indices stay valid.
        let present: Vec<Key> = keys.iter().copied().filter(|&k| self.seq.contains(k)).collect();
        for &k in present.iter().rev() {
            if self.seq.is_visible(k) {
                p.remove(self.seq.rows_before(k));
            }
            self.seq.remove(k);
        }
        let is_sep = |items: &[Item], k: Key| items[k as usize].kind == EntryKind::Separator;
        match placement {
            Placement::Before(before) => {
                let at = before.map_or(self.seq.len(), |b| self.seq.pos(b));
                let items = &self.items;
                self.seq.insert(at, keys, |k| is_sep(items, k));
            }
            Placement::At(items_at) => {
                let mut sorted: Vec<(Key, usize)> = items_at.to_vec();
                sorted.sort_by_key(|x| x.1);
                for (k, pos) in sorted {
                    let items = &self.items;
                    self.seq.insert(pos, &[k], |k| is_sep(items, k));
                }
            }
            Placement::Nowhere => {}
        }
        let len = self.seq.len();
        for &k in keys.iter().filter(|&&k| self.seq.contains(k)) {
            if self.items[k as usize].kind == EntryKind::Separator {
                touched.extend(self.block_keys(k).into_iter().filter(|x| !set.contains(x)));
            }
            let pos = self.seq.pos(k);
            // the block the entry is in (for a separator: the block it cuts) changes size
            if let Some(s) = self.seq.separator_before(pos) {
                headers.insert(s);
            }
            if pos > 0 {
                neighbours.push(self.seq.at(pos - 1));
            }
            if pos + 1 < len {
                neighbours.push(self.seq.at(pos + 1));
            }
        }
        let mut groups: HashSet<GroupId> = HashSet::new();
        for &k in keys.iter().chain(neighbours.iter()) {
            if !self.seq.contains(k) {
                continue;
            }
            if let Some(g) = self.items[k as usize].group {
                groups.insert(g);
            }
        }
        let mut updates: HashSet<Key> = headers.iter().copied().filter(|h| !set.contains(h)).collect();
        for g in groups {
            let old_first = self.groups[g as usize].as_ref().map(|gr| gr.first);
            if self.recheck_group(g) {
                let gr = self.groups[g as usize].as_ref().unwrap();
                // validity changes depth and kind of every member row
                touched.extend(gr.members.iter().copied());
                updates.extend(gr.members.iter().copied());
                updates.extend(old_first);
            }
        }
        // entries whose block changed may change depth (in a block or above the first separator)
        updates.extend(touched.iter().copied());
        touched.extend(keys.iter().copied());
        touched.extend(updates.iter().copied());
        touched.retain(|&k| self.seq.contains(k));
        self.reweigh(touched, |k| updates.contains(&k) && !set.contains(&k), p);
    }

    // ---------------------------------------------------------------- decoration as data (project file)

    /// Current decoration keyed by names (for the project file).
    pub fn decor(&self) -> Decor {
        let mut d = Decor::default();
        for k in self.seq.iter_from(0) {
            let it = &self.items[k as usize];
            if let Some(c) = it.color {
                d.colors.insert(it.name.clone(), c);
            }
            if it.kind == EntryKind::Separator && it.collapsed {
                d.collapsed.push(it.name.clone());
            }
            if it.muted {
                d.muted.push(it.name.clone());
            }
        }
        for (g, gr) in self.groups.iter().enumerate() {
            let Some(gr) = gr else { continue };
            let mut members = gr.members.clone();
            members.sort_by_key(|&k| self.seq.pos(k));
            d.groups.push(GroupDef {
                id: self.group_ids[g].clone(),
                name: gr.name.clone(),
                members: members
                    .iter()
                    .map(|&k| self.items[k as usize].name.clone())
                    .collect(),
                collapsed: gr.collapsed,
            });
        }
        d
    }

    /// Replaces the decoration (on project open). History is cleared: it refers to the previous decoration.
    /// Pieces that do not fit the canon are reported and skipped.
    pub fn apply_decor(&mut self, d: &Decor) -> (Change, Vec<DecorIssue>) {
        let mut issues = Vec::new();
        for it in &mut self.items {
            it.color = None;
            it.collapsed = false;
            it.group = None;
            it.muted = false;
        }
        self.groups.clear();
        self.group_ids.clear();
        self.undo.clear();
        self.redo.clear();
        for (name, &c) in &d.colors {
            match self.key_of(name) {
                Some(k) => self.items[k as usize].color = Some(c),
                None => issues.push(DecorIssue::UnknownEntry { name: name.clone() }),
            }
        }
        for name in &d.muted {
            match self.key_of(name) {
                Some(k) if self.items[k as usize].kind != EntryKind::Separator => {
                    self.items[k as usize].muted = true
                }
                _ => issues.push(DecorIssue::UnknownEntry { name: name.clone() }),
            }
        }
        for name in &d.collapsed {
            match self.key_of(name) {
                Some(k) if self.items[k as usize].kind == EntryKind::Separator => {
                    self.items[k as usize].collapsed = true
                }
                _ => issues.push(DecorIssue::UnknownEntry { name: name.clone() }),
            }
        }
        for g in &d.groups {
            let keys: Option<Vec<Key>> = g.members.iter().map(|m| self.key_of(m)).collect();
            let reject = |reason: &str| DecorIssue::GroupRejected {
                id: g.id.clone(),
                reason: reason.to_string(),
            };
            let Some(keys) = keys else {
                issues.push(reject("a member is not in the list"));
                continue;
            };
            // a stored group may be broken (an entry was dropped into it): it is kept, shown as plain rows
            match self.group_rule(&keys, false) {
                Err(e) => issues.push(reject(e)),
                Ok(members) => {
                    let id = self.groups.len() as GroupId;
                    for &k in &members {
                        self.items[k as usize].group = Some(id);
                    }
                    self.groups.push(Some(Group {
                        name: g.name.clone(),
                        first: members[0],
                        members,
                        collapsed: g.collapsed,
                        valid: true,
                    }));
                    self.group_ids.push(g.id.clone());
                    self.recheck_group(id);
                }
            }
        }
        let filter = self.filter.clone();
        (self.set_filter(filter), issues)
    }

    // ---------------------------------------------------------------- view operations (not in history)

    /// Collapses or expands a separator's block.
    pub fn set_collapsed(&mut self, sep: Key, collapsed: bool) -> Result<Option<Change>, OrderError> {
        self.check_key(sep)?;
        let it = &mut self.items[sep as usize];
        if it.kind != EntryKind::Separator || it.collapsed == collapsed {
            return Ok(None);
        }
        it.collapsed = collapsed;
        let mut p = Patches::default();
        let mut keys = self.block_keys(sep);
        keys.push(sep);
        self.reweigh(keys, |k| k == sep, &mut p);
        Ok(Some(self.finish(p)))
    }

    /// Collapses (one row) or expands a logical group.
    pub fn set_group_collapsed(&mut self, g: GroupId, collapsed: bool) -> Result<Option<Change>, OrderError> {
        let Some(gr) = self.groups.get_mut(g as usize).and_then(Option::as_mut) else {
            return Err(OrderError::Group("no such group"));
        };
        if gr.collapsed == collapsed {
            return Ok(None);
        }
        gr.collapsed = collapsed;
        let (members, first) = (gr.members.clone(), gr.first);
        let mut p = Patches::default();
        self.reweigh(members, |k| k == first, &mut p);
        Ok(Some(self.finish(p)))
    }

    /// Replaces the filter: one pass over the list, then a [`Patch::Reset`].
    pub fn set_filter(&mut self, mut filter: Filter) -> Change {
        filter.text = filter.text.to_lowercase();
        self.filter = filter;
        let mut vis = vec![false; self.items.len()];
        let mut collapsed = false;
        for k in self.seq.iter_from(0) {
            if self.items[k as usize].kind == EntryKind::Separator {
                collapsed = self.items[k as usize].collapsed;
            }
            vis[k as usize] = self.want_with(k, collapsed);
        }
        self.seq.reflag_all(|k| vis[k as usize]);
        self.cursor.shown = self.effective_focus();
        self.revision += 1;
        let len = self.seq.visible_len() as u32;
        Change {
            revision: self.revision,
            len,
            patches: vec![Patch::Reset { len }],
        }
    }

    // ---------------------------------------------------------------- reading

    /// Rows `[start, start + count)` of the current view.
    pub fn rows(&self, start: usize, count: usize) -> Vec<Row<'_>> {
        self.seq
            .rows_from(start)
            .take(count)
            .map(|k| self.row_of(k))
            .collect()
    }

    fn row_of(&self, k: Key) -> Row<'_> {
        let it = &self.items[k as usize];
        let pos = self.seq.pos(k);
        let in_block = self.owner(k).is_some();
        let mut row = Row {
            key: k,
            pos: pos as u32,
            kind: match it.kind {
                EntryKind::Mod => RowKind::Mod,
                EntryKind::Separator => RowKind::Separator,
                EntryKind::Foreign => RowKind::Foreign,
            },
            name: &it.name,
            enabled: it.enabled,
            depth: in_block as u8,
            collapsed: it.collapsed,
            color: it.color,
            size: 0,
            group: it.group,
            muted: it.muted,
            focused: self.effective_focus() == Some(k),
            selected: self.cursor.selected.contains(&k),
        };
        if it.kind == EntryKind::Separator {
            row.size = (self.seq.block_end(pos + 1) - pos - 1) as u32;
        } else if let Some((_, g)) = self.group(k) {
            if g.valid && g.collapsed && g.first == k {
                row.kind = RowKind::Group;
                row.name = &g.name;
                row.collapsed = true;
                row.size = g.members.len() as u32;
                row.enabled = g.members.iter().all(|&m| self.items[m as usize].enabled);
            } else if g.valid {
                row.depth += 1;
            }
        }
        row
    }
}

enum Placement<'a> {
    Before(Option<Key>),
    At(&'a [(Key, usize)]),
    /// Taken out of the list (kept as a tombstone for undo).
    Nowhere,
}

#[cfg(test)]
impl OrderBook {
    /// Rows recomputed from scratch by a plain scan (test oracle for the incremental path).
    pub(super) fn naive_rows(&self) -> Vec<Key> {
        let order: Vec<Key> = self.seq.iter_from(0).collect();
        let mut pos = vec![usize::MAX; self.items.len()];
        for (i, &k) in order.iter().enumerate() {
            pos[k as usize] = i;
        }
        let group_ok = |g: &Group| {
            let ps: Vec<usize> = g.members.iter().map(|&m| pos[m as usize]).collect();
            let (lo, hi) = (*ps.iter().min().unwrap(), *ps.iter().max().unwrap());
            (hi - lo + 1 == ps.len(), order[lo])
        };
        let mut out = Vec::new();
        let mut collapsed = false;
        for &k in &order {
            let it = &self.items[k as usize];
            if it.kind == EntryKind::Separator {
                collapsed = it.collapsed;
                out.push(k);
                continue;
            }
            if collapsed || !self.passes_filter(it) {
                continue;
            }
            if let Some(g) = it.group.and_then(|g| self.groups[g as usize].as_ref()) {
                let (valid, first) = group_ok(g);
                assert_eq!((valid, first), (g.valid, g.first), "group state drifted");
                if valid && g.collapsed && first != k {
                    continue;
                }
            }
            out.push(k);
        }
        out
    }

    pub(super) fn check(&self) {
        self.seq.check();
    }
    pub(super) fn groups_of_test(&self) -> Vec<GroupId> {
        (0..self.groups.len() as GroupId)
            .filter(|&g| self.groups[g as usize].is_some())
            .collect()
    }
}
