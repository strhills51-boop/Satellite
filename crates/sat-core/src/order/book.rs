//! [`OrderBook`] — the live canon of a list (any size) and its views, changed only by operations that @sat:order.book
//! return **row patches** per view and the **change set** of the truth (ADR 0008, 0009, 0010). A UI never
//! re-renders a list: each window keeps its row count and visible window and applies its view's [`Patch`]es.
//!
//! Three layers, never mixed:
//! - **canon** — order and enabled flags (what goes to `modlist.txt`);
//! - **decoration** — colour, logical groups, muted: stored next to the canon, never reorder it;
//! - **views** — one per window ([`ViewId`]; [`MAIN`] always open): filter, collapsed separators and groups,
//!   keyboard cursor. The main view's collapse state is the stored default; a new view starts from it.
//!
//! Owns: the live canon and decoration of a list, its operations, undo/redo, the views over it, rows and row patches.
//! Not here: positions and per-view visibility counters (`@sat:order.seq`), the cursor semantics
//! (`@sat:order.cursor`), stored decoration (`@sat:order.decor`).

use std::collections::HashSet;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use super::cursor::Cursor;
use super::decor::{CardStyle, Decor, DecorIssue, GroupDef, Marks};
use super::seq::{Key, Layer, Seq};
use crate::canon::{Entry, EntryKind, Modlist};
use crate::engine::{self, Keys, Truth};

pub type GroupId = u32;

/// A view of the book (= a layer of the sequence).
pub type ViewId = Layer;

/// The view every book has; its collapse state is what the project stores.
pub const MAIN: ViewId = 0;

// @sat:order.book.patch
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

/// Result of an operation: what changed in the truth and how each affected view's rows changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    /// Revision of the book after the change.
    pub revision: u64,
    /// Truth changes for the engine (empty for view-only operations: filter, fold, cursor).
    pub truth: Vec<engine::Change>,
    /// One entry per view the operation touched (every open view for canon and decoration changes).
    pub views: Vec<ViewChange>,
}

/// Row patches of one view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewChange {
    pub view: ViewId,
    /// Row count after the change.
    pub len: u32,
    pub patches: Vec<Patch>,
}

impl Change {
    pub fn view(&self, v: ViewId) -> Option<&ViewChange> {
        self.views.iter().find(|c| c.view == v)
    }
    /// The main view's part (always present for canon and decoration changes).
    pub fn main(&self) -> &ViewChange {
        self.view(MAIN).expect("change does not touch the main view")
    }
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
    #[error("no such view {0}")]
    UnknownView(ViewId),
    #[error("too many views open")]
    TooManyViews,
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
    /// Card style (ADR 0012), if the entry has one.
    pub style: Option<&'a CardStyle>,
    /// Tags, stickers, note.
    pub marks: Option<&'a Marks>,
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
    pub(super) color: Option<u32>,
    pub(super) group: Option<GroupId>,
    pub(super) muted: bool,
    /// Index into the book's interned card styles (ADR 0012).
    pub(super) style: Option<u32>,
    /// Index into the book's interned marks (tags, stickers, note).
    pub(super) marks: Option<u32>,
}

#[derive(Debug, Clone)]
pub(super) struct Group {
    name: String,
    pub(super) members: Vec<Key>,
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
    /// Card style by interned id.
    Style(Vec<(Key, Option<u32>)>),
    /// Marks by interned id.
    Marks(Vec<(Key, Option<u32>)>),
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

/// What an operation produces while it runs: patches per view, truth changes.
pub(super) struct Out {
    pub(super) views: Vec<(ViewId, Patches)>,
    truth: Vec<engine::Change>,
}

impl Out {
    pub(super) fn p(&mut self, v: ViewId) -> &mut Patches {
        &mut self
            .views
            .iter_mut()
            .find(|x| x.0 == v)
            .expect("view in output")
            .1
    }
    fn truth(&mut self, truth: Truth, keys: impl IntoIterator<Item = Key>) {
        self.truth.push(engine::Change {
            truth,
            keys: Keys::some(keys.into_iter().collect()),
        });
    }
}

/// State of one view.
#[derive(Debug, Clone, Default)]
pub(super) struct View {
    pub(super) filter: Filter,
    /// Collapsed separators.
    pub(super) collapsed: FxHashSet<Key>,
    /// Logical groups shown expanded (groups are collapsed by default).
    pub(super) open_groups: FxHashSet<GroupId>,
    pub(super) cursor: Cursor,
}

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
    /// Views by layer (`None` = closed).
    pub(super) views: Vec<Option<View>>,
    /// Interned card styles: thousands of entries share a handful of styles (ADR 0012).
    styles: Vec<CardStyle>,
    style_ids: FxHashMap<CardStyle, u32>,
    /// Entries that have a style (the flow layout lays out only their blocks entry by entry).
    pub(super) styled: FxHashSet<Key>,
    /// Interned marks (most entries share none; tag sets repeat).
    marks: Vec<Marks>,
    mark_ids: FxHashMap<Marks, u32>,
    revision: u64,
    undo: Vec<(Op, Op)>,
    redo: Vec<(Op, Op)>,
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
                color: None,
                group: None,
                muted: false,
                style: None,
                marks: None,
            })
            .collect();
        let by_name = items
            .iter()
            .enumerate()
            .map(|(i, it)| (it.folded.clone(), i as Key))
            .collect();
        let keys: Vec<Key> = (0..items.len() as Key).collect();
        let mut seq = Seq::from_keys(&keys, |k| items[k as usize].kind == EntryKind::Separator);
        let main = seq.add_layer().expect("a fresh sequence has free layers");
        debug_assert_eq!(main, MAIN);
        seq.reflag_all(MAIN, |_| true);
        Self {
            items,
            by_name,
            seq,
            groups: Vec::new(),
            group_ids: Vec::new(),
            views: vec![Some(View::default())],
            styles: Vec::new(),
            style_ids: FxHashMap::default(),
            styled: FxHashSet::default(),
            marks: Vec::new(),
            mark_ids: FxHashMap::default(),
            revision: 0,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    // @sat:order.book.views
    /// Opens a view starting from the main view's collapse state, with no filter and no cursor.
    pub fn open_view(&mut self) -> Result<ViewId, OrderError> {
        let v = self.seq.add_layer().ok_or(OrderError::TooManyViews)?;
        let main = self.view(MAIN);
        let state = View {
            collapsed: main.collapsed.clone(),
            open_groups: main.open_groups.clone(),
            ..View::default()
        };
        if self.views.len() <= v {
            self.views.resize(v + 1, None);
        }
        self.views[v] = Some(state);
        self.refill_view(v);
        Ok(v)
    }

    /// Closes a view (not the main one).
    pub fn close_view(&mut self, v: ViewId) -> Result<(), OrderError> {
        if v == MAIN || self.views.get(v).is_none_or(Option::is_none) {
            return Err(OrderError::UnknownView(v));
        }
        self.views[v] = None;
        self.seq.drop_layer(v);
        Ok(())
    }

    /// Open views.
    pub fn view_ids(&self) -> Vec<ViewId> {
        (0..self.views.len())
            .filter(|&v| self.views[v].is_some())
            .collect()
    }

    pub(super) fn view(&self, v: ViewId) -> &View {
        self.views[v].as_ref().expect("open view")
    }

    pub(super) fn view_mut(&mut self, v: ViewId) -> &mut View {
        self.views[v].as_mut().expect("open view")
    }

    pub(super) fn check_view(&self, v: ViewId) -> Result<(), OrderError> {
        if self.views.get(v).is_some_and(Option::is_some) {
            Ok(())
        } else {
            Err(OrderError::UnknownView(v))
        }
    }

    /// An output collecting patches of every open view.
    pub(super) fn out_all(&self) -> Out {
        Out {
            views: self
                .view_ids()
                .into_iter()
                .map(|v| (v, Patches::default()))
                .collect(),
            truth: Vec::new(),
        }
    }

    /// An output for one view.
    pub(super) fn out_one(&self, v: ViewId) -> Out {
        Out {
            views: vec![(v, Patches::default())],
            truth: Vec::new(),
        }
    }

    /// Entries `(name, kind, enabled)` in display order, without copying (projections read the canon here).
    pub fn entries(&self) -> impl Iterator<Item = (&str, EntryKind, bool)> + '_ {
        self.seq.iter_from(0).map(|k| {
            let it = &self.items[k as usize];
            (it.name.as_str(), it.kind, it.enabled)
        })
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
    pub fn row_count(&self, v: ViewId) -> usize {
        self.seq.visible_len(v)
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
    /// Name of an entry (kept after the entry is taken out of the list, for undo).
    pub fn name(&self, k: Key) -> &str {
        &self.items[k as usize].name
    }

    /// The separator whose block holds `k` (none before the first separator).
    pub fn separator_of(&self, k: Key) -> Option<Key> {
        self.seq.separator_before(self.seq.pos(k))
    }

    pub fn kind(&self, k: Key) -> EntryKind {
        self.items[k as usize].kind
    }
    pub fn enabled(&self, k: Key) -> bool {
        self.items[k as usize].enabled
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

    fn passes_filter(&self, it: &Item, v: ViewId) -> bool {
        let f = &self.view(v).filter;
        (f.text.is_empty() || it.folded.contains(&f.text))
            && f.enabled.is_none_or(|e| e == it.enabled)
            && f.color.is_none_or(|c| it.color == Some(c))
            && (f.show_muted || !it.muted)
    }

    /// A logical group is shown as one row in view `v`.
    pub(super) fn group_collapsed(&self, g: GroupId, v: ViewId) -> bool {
        !self.view(v).open_groups.contains(&g)
    }

    /// Whether entry `k` is a row of view `v`, given `block_collapsed` = the state of its block's separator.
    fn want_with(&self, k: Key, v: ViewId, block_collapsed: bool) -> bool {
        let it = &self.items[k as usize];
        if it.kind == EntryKind::Separator {
            return true;
        }
        if block_collapsed || !self.passes_filter(it, v) {
            return false;
        }
        match self.group(k) {
            Some((gid, g)) => !(g.valid && self.group_collapsed(gid, v) && g.first != k),
            None => true,
        }
    }

    fn want(&self, k: Key, v: ViewId) -> bool {
        let collapsed = self.items[k as usize].kind != EntryKind::Separator
            && self
                .seq
                .separator_before(self.seq.pos(k))
                .is_some_and(|s| self.view(v).collapsed.contains(&s));
        self.want_with(k, v, collapsed)
    }

    /// Re-evaluates rows of `keys` (any order; sorted here) in every view of `out`, emitting patches;
    /// `update(k)` marks a row whose content changed in place.
    pub(super) fn reweigh(
        &mut self,
        keys: impl IntoIterator<Item = Key>,
        update: impl Fn(Key) -> bool,
        out: &mut Out,
    ) {
        let mut ks: Vec<(usize, Key)> = keys.into_iter().map(|k| (self.seq.pos(k), k)).collect();
        ks.sort_unstable();
        ks.dedup();
        for i in 0..out.views.len() {
            let v = out.views[i].0;
            for &(_, k) in &ks {
                let (had, want) = (self.seq.is_visible(k, v), self.want(k, v));
                let p = &mut out.views[i].1;
                match (had, want) {
                    (false, true) => {
                        self.seq.set_visible(k, v, true);
                        p.insert(self.seq.rows_before(k, v));
                    }
                    (true, false) => {
                        p.remove(self.seq.rows_before(k, v));
                        self.seq.set_visible(k, v, false);
                    }
                    (true, true) if update(k) => p.update(self.seq.rows_before(k, v)),
                    _ => {}
                }
            }
        }
    }

    /// Recomputes view `v`'s rows from scratch (no patches): opening a view, a new filter, new decoration.
    fn refill_view(&mut self, v: ViewId) {
        let mut vis = vec![false; self.items.len()];
        let mut collapsed = false;
        for k in self.seq.iter_from(0) {
            if self.items[k as usize].kind == EntryKind::Separator {
                collapsed = self.view(v).collapsed.contains(&k);
            }
            vis[k as usize] = self.want_with(k, v, collapsed);
        }
        self.seq.reflag_all(v, |k| vis[k as usize]);
        let focus = self.effective_focus(v);
        self.view_mut(v).cursor.shown = focus;
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

    pub(super) fn finish(&mut self, mut out: Out) -> Change {
        for i in 0..out.views.len() {
            let v = out.views[i].0;
            self.sync_focus_mark(v, &mut out.views[i].1);
        }
        self.seq.compact();
        self.revision += 1;
        Change {
            revision: self.revision,
            truth: out.truth,
            views: out
                .views
                .into_iter()
                .map(|(v, p)| ViewChange {
                    view: v,
                    len: self.seq.visible_len(v) as u32,
                    patches: p.0,
                })
                .collect(),
        }
    }

    fn record(&mut self, fwd: Op, inv: Op) {
        self.undo.push((fwd, inv));
        self.redo.clear();
    }

    // ---------------------------------------------------------------- canon operations (undoable)

    // @sat:order.book.move
    /// Moves entries right before `before` (`None` = end), keeping their relative order. A separator moves with
    /// its block; a collapsed group row moves with its members. A target inside a group snaps to the group's
    /// start unless the moved entries include its members. `Ok(None)` = nothing changed.
    pub fn move_keys(&mut self, keys: &[Key], before: Option<Key>) -> Result<Option<Change>, OrderError> {
        let fwd = Op::Move {
            keys: keys.to_vec(),
            before,
        };
        let mut out = self.out_all();
        let Some(inv) = self.apply(&fwd, &mut out)? else {
            return Ok(None);
        };
        self.record(fwd, inv);
        Ok(Some(self.finish(out)))
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

    // @sat:order.book.style
    /// Sets (or clears with `None`) the card style of entries; undoable. Identical styles share one interned copy.
    pub fn set_style(
        &mut self,
        keys: &[Key],
        style: Option<CardStyle>,
    ) -> Result<Option<Change>, OrderError> {
        for &k in keys {
            self.check_key(k)?;
        }
        let id = style.map(|st| self.intern_style(st));
        self.apply_recorded(Op::Style(keys.iter().map(|&k| (k, id)).collect()))
    }

    // @sat:order.book.patch-style
    /// Changes the card style of each entry by `f(current)` in one undoable step: an editor changes one field on
    /// many entries and keeps their other fields. A result equal to the default style clears the style.
    pub fn patch_style<E>(
        &mut self,
        keys: &[Key],
        mut f: impl FnMut(Option<&CardStyle>) -> Result<Option<CardStyle>, E>,
    ) -> Result<Option<Change>, PatchStyleError<E>> {
        let mut each = Vec::with_capacity(keys.len());
        for &k in keys {
            self.check_key(k).map_err(PatchStyleError::Order)?;
            let next = f(self.style(k)).map_err(PatchStyleError::Patch)?;
            let next = next.filter(|st| *st != CardStyle::default());
            each.push((k, next.map(|st| self.intern_style(st))));
        }
        self.apply_recorded(Op::Style(each))
            .map_err(PatchStyleError::Order)
    }

    // @sat:order.book.marks
    /// Changes the marks of each entry by `f(current)` in one undoable step (add a tag to a selection, edit a note).
    /// The result is normalized; empty marks are removed.
    pub fn patch_marks(
        &mut self,
        keys: &[Key],
        mut f: impl FnMut(Marks) -> Marks,
    ) -> Result<Option<Change>, OrderError> {
        let mut each = Vec::with_capacity(keys.len());
        for &k in keys {
            self.check_key(k)?;
            let next = f(self.marks(k).cloned().unwrap_or_default()).normalized();
            let id = (!next.is_empty()).then(|| self.intern_marks(next));
            each.push((k, id));
        }
        self.apply_recorded(Op::Marks(each))
    }

    /// Marks of an entry.
    pub fn marks(&self, k: Key) -> Option<&Marks> {
        self.items[k as usize].marks.map(|i| &self.marks[i as usize])
    }

    fn intern_marks(&mut self, m: Marks) -> u32 {
        if let Some(&i) = self.mark_ids.get(&m) {
            return i;
        }
        let i = self.marks.len() as u32;
        self.marks.push(m.clone());
        self.mark_ids.insert(m, i);
        i
    }

    /// Every tag in use, with the number of entries carrying it (for a tag list and filters).
    pub fn tags_in_use(&self) -> Vec<(String, usize)> {
        let mut count: std::collections::BTreeMap<String, (String, usize)> = Default::default();
        for it in &self.items {
            if let Some(i) = it.marks {
                for t in &self.marks[i as usize].tags {
                    count.entry(t.to_lowercase()).or_insert_with(|| (t.clone(), 0)).1 += 1;
                }
            }
        }
        count.into_values().collect()
    }

    /// Card style of an entry.
    pub fn style(&self, k: Key) -> Option<&CardStyle> {
        self.items[k as usize].style.map(|i| &self.styles[i as usize])
    }

    fn intern_style(&mut self, st: CardStyle) -> u32 {
        if let Some(&i) = self.style_ids.get(&st) {
            return i;
        }
        let i = self.styles.len() as u32;
        self.styles.push(st.clone());
        self.style_ids.insert(st, i);
        i
    }

    // @sat:order.book.muted
    /// Mutes (hides unless the filter shows muted entries) or unmutes entries. Separators cannot be muted.
    pub fn set_muted(&mut self, keys: &[Key], muted: bool) -> Result<Option<Change>, OrderError> {
        for &k in keys {
            self.check_key(k)?;
        }
        self.apply_recorded(Op::Muted(keys.iter().map(|&k| (k, muted)).collect()))
    }

    // @sat:order.book.group
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

    // @sat:order.book.insert
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
                    color: None,
                    group: None,
                    muted: false,
                    style: None,
                    marks: None,
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

    // @sat:order.book.remove
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

    // @sat:order.book.rename
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
        let mut out = self.out_all();
        let Some(inv) = self.apply(&fwd, &mut out)? else {
            return Ok(None);
        };
        self.record(fwd, inv);
        Ok(Some(self.finish(out)))
    }

    pub fn undo(&mut self) -> Option<Change> {
        let (fwd, inv) = self.undo.pop()?;
        let mut out = self.out_all();
        self.apply(&inv, &mut out).expect("inverse operations are valid");
        self.redo.push((fwd, inv));
        Some(self.finish(out))
    }

    pub fn redo(&mut self) -> Option<Change> {
        let (fwd, inv) = self.redo.pop()?;
        let mut out = self.out_all();
        self.apply(&fwd, &mut out).expect("redone operations are valid");
        self.undo.push((fwd, inv));
        Some(self.finish(out))
    }

    /// Applies an operation; returns its inverse, or `None` when nothing changed.
    fn apply(&mut self, op: &Op, p: &mut Out) -> Result<Option<Op>, OrderError> {
        Ok(match op {
            Op::Move { keys, before } => self.apply_move(keys, *before, p)?,
            Op::Place(items) => {
                let mut keys: Vec<Key> = items.iter().map(|x| x.0).collect();
                keys.sort_by_key(|&k| self.seq.pos(k));
                let inv: Vec<(Key, usize)> = keys.iter().map(|&k| (k, self.seq.pos(k))).collect();
                self.relocate(&keys, Placement::At(items), p);
                p.truth(Truth::Canon, keys);
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
                p.truth(Truth::Canon, touched.iter().copied());
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
                p.truth(Truth::Decoration, touched.iter().copied());
                self.after_content_change(touched, p);
                (!inv.is_empty()).then_some(Op::Color(inv))
            }
            Op::Style(each) => {
                let mut inv = Vec::new();
                let mut touched = HashSet::new();
                for &(k, st) in each {
                    let it = &mut self.items[k as usize];
                    if it.style != st {
                        inv.push((k, it.style));
                        it.style = st;
                        touched.insert(k);
                        if st.is_some() {
                            self.styled.insert(k);
                        } else {
                            self.styled.remove(&k);
                        }
                    }
                }
                p.truth(Truth::Decoration, touched.iter().copied());
                self.after_content_change(touched, p);
                (!inv.is_empty()).then_some(Op::Style(inv))
            }
            Op::Marks(each) => {
                let mut inv = Vec::new();
                let mut touched = HashSet::new();
                for &(k, m) in each {
                    let it = &mut self.items[k as usize];
                    if it.marks != m {
                        inv.push((k, it.marks));
                        it.marks = m;
                        touched.insert(k);
                    }
                }
                p.truth(Truth::Decoration, touched.iter().copied());
                self.after_content_change(touched, p);
                (!inv.is_empty()).then_some(Op::Marks(inv))
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
                p.truth(Truth::Decoration, touched.iter().copied());
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
                    valid: false,
                    first: members[0],
                });
                for &k in members {
                    self.items[k as usize].group = Some(id);
                }
                for view in self.views.iter_mut().flatten() {
                    if *collapsed {
                        view.open_groups.remove(&id);
                    } else {
                        view.open_groups.insert(id);
                    }
                }
                self.recheck_group(id);
                p.truth(Truth::Decoration, members.iter().copied());
                // members change depth, the first one becomes the group row
                self.reweigh(members.clone(), |_| true, p);
                Some(Op::Dissolve(id))
            }
            Op::Put { keys, before } => {
                self.relocate(keys, Placement::Before(*before), p);
                self.name_keys(keys, true);
                p.truth(Truth::Canon, keys.iter().copied());
                Some(Op::Take(keys.clone()))
            }
            Op::Restore(items) => {
                let keys: Vec<Key> = items.iter().map(|x| x.0).collect();
                self.relocate(&keys, Placement::At(items), p);
                self.name_keys(&keys, true);
                p.truth(Truth::Canon, keys.iter().copied());
                Some(Op::Take(keys))
            }
            Op::Take(keys) => {
                let mut keys = keys.clone();
                keys.sort_by_key(|&k| self.seq.pos(k));
                let inv: Vec<(Key, usize)> = keys.iter().map(|&k| (k, self.seq.pos(k))).collect();
                self.relocate(&keys, Placement::Nowhere, p);
                self.name_keys(&keys, false);
                self.cursor_forget(&keys, inv.first().map_or(0, |x| x.1), p);
                p.truth(Truth::Canon, keys.iter().copied());
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
                p.truth(Truth::Canon, [*k]);
                self.after_content_change(HashSet::from([*k]), p);
                Some(Op::Rename(*k, old))
            }
            Op::Dissolve(id) => {
                let g = self.groups[*id as usize].take().expect("group exists");
                for &k in &g.members {
                    self.items[k as usize].group = None;
                }
                let collapsed = !self.view(MAIN).open_groups.contains(id);
                for view in self.views.iter_mut().flatten() {
                    view.open_groups.remove(id);
                }
                self.reweigh(g.members.clone(), |_| true, p);
                p.truth(Truth::Decoration, g.members.iter().copied());
                Some(Op::MakeGroup {
                    id: *id,
                    name: g.name,
                    members: g.members,
                    collapsed,
                })
            }
        })
    }

    /// Rows whose content depends on changed entries: the entries themselves and their groups' rows.
    pub(super) fn after_content_change(&mut self, touched: HashSet<Key>, p: &mut Out) {
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
        p: &mut Out,
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
                    // a group row moves its members (collapsed in the main view: what the canon user sees)
                    if let Some((gid, g)) = self.group(k)
                        && g.valid
                        && self.group_collapsed(gid, MAIN)
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
        p.truth(Truth::Canon, keys.iter().copied());
        Ok(Some(inv))
    }

    /// Takes `keys` (in canon order) out and puts them back per `placement`, emitting row patches for every
    /// entry whose row changed: the moved entries, entries whose block changed, group rows, separator headers.
    fn relocate(&mut self, keys: &[Key], placement: Placement<'_>, out: &mut Out) {
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
            for (v, p) in out.views.iter_mut() {
                if self.seq.is_visible(k, *v) {
                    p.remove(self.seq.rows_before(k, *v));
                }
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
        self.reweigh(touched, |k| updates.contains(&k) && !set.contains(&k), out);
    }

    // ---------------------------------------------------------------- decoration as data (project file)

    // @sat:order.book.decor
    /// Current decoration keyed by names (for the project file).
    pub fn decor(&self) -> Decor {
        let mut d = Decor::default();
        for k in self.seq.iter_from(0) {
            let it = &self.items[k as usize];
            if let Some(c) = it.color {
                d.colors.insert(it.name.clone(), c);
            }
            if it.kind == EntryKind::Separator && self.view(MAIN).collapsed.contains(&k) {
                d.collapsed.push(it.name.clone());
            }
            if it.muted {
                d.muted.push(it.name.clone());
            }
            if let Some(i) = it.style {
                d.styles.insert(it.name.clone(), self.styles[i as usize].clone());
            }
            if let Some(i) = it.marks {
                d.marks.insert(it.name.clone(), self.marks[i as usize].clone());
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
                collapsed: self.group_collapsed(g as GroupId, MAIN),
            });
        }
        d
    }

    /// Replaces the decoration (on project open). History is cleared: it refers to the previous decoration.
    /// Every view takes the stored collapse state. Pieces that do not fit the canon are reported and skipped.
    pub fn apply_decor(&mut self, d: &Decor) -> (Change, Vec<DecorIssue>) {
        let mut issues = Vec::new();
        for it in &mut self.items {
            it.color = None;
            it.group = None;
            it.muted = false;
            it.style = None;
            it.marks = None;
        }
        self.styled.clear();
        let mut collapsed: FxHashSet<Key> = FxHashSet::default();
        let mut open_groups: FxHashSet<GroupId> = FxHashSet::default();
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
        for (name, st) in &d.styles {
            match self.key_of(name) {
                Some(k) => {
                    let id = self.intern_style(st.clone());
                    self.items[k as usize].style = Some(id);
                    self.styled.insert(k);
                }
                None => issues.push(DecorIssue::UnknownEntry { name: name.clone() }),
            }
        }
        for (name, m) in &d.marks {
            match self.key_of(name) {
                Some(k) => {
                    let m = m.clone().normalized();
                    if !m.is_empty() {
                        let id = self.intern_marks(m);
                        self.items[k as usize].marks = Some(id);
                    }
                }
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
                    collapsed.insert(k);
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
                        valid: true,
                    }));
                    if !g.collapsed {
                        open_groups.insert(id);
                    }
                    self.group_ids.push(g.id.clone());
                    self.recheck_group(id);
                }
            }
        }
        self.revision += 1;
        let mut views = Vec::new();
        for v in self.view_ids() {
            let view = self.view_mut(v);
            view.collapsed = collapsed.clone();
            view.open_groups = open_groups.clone();
            self.refill_view(v);
            let len = self.seq.visible_len(v) as u32;
            views.push(ViewChange {
                view: v,
                len,
                patches: vec![Patch::Reset { len }],
            });
        }
        let change = Change {
            revision: self.revision,
            truth: vec![engine::Change {
                truth: Truth::Decoration,
                keys: Keys::All,
            }],
            views,
        };
        (change, issues)
    }

    // ---------------------------------------------------------------- view operations (not in history)

    /// Collapses or expands a separator's block in view `v`. The main view's state is what the project stores.
    pub fn set_collapsed(
        &mut self,
        v: ViewId,
        sep: Key,
        collapsed: bool,
    ) -> Result<Option<Change>, OrderError> {
        self.check_view(v)?;
        self.check_key(sep)?;
        if self.items[sep as usize].kind != EntryKind::Separator
            || self.view(v).collapsed.contains(&sep) == collapsed
        {
            return Ok(None);
        }
        let set = &mut self.view_mut(v).collapsed;
        if collapsed {
            set.insert(sep);
        } else {
            set.remove(&sep);
        }
        let mut out = self.out_one(v);
        let mut keys = self.block_keys(sep);
        keys.push(sep);
        self.reweigh(keys, |k| k == sep, &mut out);
        Ok(Some(self.finish(out)))
    }

    /// Collapses (one row) or expands a logical group in view `v`.
    pub fn set_group_collapsed(
        &mut self,
        v: ViewId,
        g: GroupId,
        collapsed: bool,
    ) -> Result<Option<Change>, OrderError> {
        self.check_view(v)?;
        let Some(gr) = self.groups.get(g as usize).and_then(Option::as_ref) else {
            return Err(OrderError::Group("no such group"));
        };
        let (members, first) = (gr.members.clone(), gr.first);
        if self.group_collapsed(g, v) == collapsed {
            return Ok(None);
        }
        let open = &mut self.view_mut(v).open_groups;
        if collapsed {
            open.remove(&g);
        } else {
            open.insert(g);
        }
        let mut out = self.out_one(v);
        self.reweigh(members, |k| k == first, &mut out);
        Ok(Some(self.finish(out)))
    }

    // @sat:order.book.filter
    /// Replaces the filter of view `v`: one pass over the list, then a [`Patch::Reset`] for that view.
    pub fn set_filter(&mut self, v: ViewId, mut filter: Filter) -> Result<Change, OrderError> {
        self.check_view(v)?;
        filter.text = filter.text.to_lowercase();
        self.view_mut(v).filter = filter;
        self.refill_view(v);
        self.revision += 1;
        let len = self.seq.visible_len(v) as u32;
        Ok(Change {
            revision: self.revision,
            truth: Vec::new(),
            views: vec![ViewChange {
                view: v,
                len,
                patches: vec![Patch::Reset { len }],
            }],
        })
    }

    // ---------------------------------------------------------------- reading

    // @sat:order.book.rows
    /// Rows `[start, start + count)` of view `v` (empty for a closed view).
    pub fn rows(&self, v: ViewId, start: usize, count: usize) -> Vec<Row<'_>> {
        if self.check_view(v).is_err() {
            return Vec::new();
        }
        let focus = self.effective_focus(v);
        self.seq
            .rows_from(start, v)
            .take(count)
            .map(|k| self.row_of(k, v, focus))
            .collect()
    }

    fn row_of(&self, k: Key, v: ViewId, focus: Option<Key>) -> Row<'_> {
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
            collapsed: self.view(v).collapsed.contains(&k),
            color: it.color,
            size: 0,
            group: it.group,
            muted: it.muted,
            style: it.style.map(|i| &self.styles[i as usize]),
            marks: it.marks.map(|i| &self.marks[i as usize]),
            focused: focus == Some(k),
            selected: self.view(v).cursor.selected.contains(&k),
        };
        if it.kind == EntryKind::Separator {
            row.size = (self.seq.block_end(pos + 1) - pos - 1) as u32;
        } else if let Some((gid, g)) = self.group(k) {
            if g.valid && self.group_collapsed(gid, v) && g.first == k {
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
    pub(super) fn naive_rows(&self, v: ViewId) -> Vec<Key> {
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
                collapsed = self.view(v).collapsed.contains(&k);
                out.push(k);
                continue;
            }
            if collapsed || !self.passes_filter(it, v) {
                continue;
            }
            if let Some(gid) = it.group
                && let Some(g) = self.groups[gid as usize].as_ref()
            {
                let (valid, first) = group_ok(g);
                assert_eq!((valid, first), (g.valid, g.first), "group state drifted");
                if valid && self.group_collapsed(gid, v) && first != k {
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

/// Why [`OrderBook::patch_style`] refused: a bad key, or the patch did not apply to an entry's style.
#[derive(Debug)]
pub enum PatchStyleError<E> {
    Order(OrderError),
    Patch(E),
}
