//! Keyboard cursor of a view: **focus** (one entry, where keys act), **anchor** (where a Shift range starts) @sat:order.cursor
//! and **selection** (a set). Every view has its own cursor (ADR 0009). All three are entry keys, not row
//! numbers, so they survive moves, filters and folds; every change returns row patches (ADR 0008).
//!
//! Layout the UI binds (decided here so every client behaves the same):
//! - ↑ ↓ PgUp PgDn Home End — [`OrderBook::navigate`] with [`Select::Replace`]; with Shift — [`Select::Extend`];
//!   with Ctrl — [`Select::FocusOnly`]; Ctrl+click — [`Select::Toggle`]; Space — [`OrderBook::toggle_focused`];
//! - Alt+↑ ↓ — [`OrderBook::shift_selection`] by one row; Alt+Shift+↑ ↓ — to the start / end of the block;
//! - ← → — [`OrderBook::fold_focused`]: collapse / expand a separator or group, ← on a mod jumps to its separator;
//! - letters — [`OrderBook::jump_to`]; Ctrl+A — [`OrderBook::select_all`]; Esc — [`OrderBook::clear_selection`].
//!
//! Owns: keyboard focus, anchor, selection and the key layout semantics.

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use super::book::{Change, OrderBook, OrderError, Out, Patches, ViewId};
use super::seq::Key;
use crate::canon::EntryKind;

/// Above this many changed rows a selection change repaints the whole list with one patch.
const BULK: usize = 4096;

#[derive(Debug, Clone, Default)]
pub(super) struct Cursor {
    pub(super) focus: Option<Key>,
    pub(super) anchor: Option<Key>,
    pub(super) selected: FxHashSet<Key>,
    /// Row the UI was last told carries the focus mark (focus or, when hidden, its visible stand-in).
    pub(super) shown: Option<Key>,
}

/// Where the focus goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Nav {
    Up,
    Down,
    PageUp(u32),
    PageDown(u32),
    Home,
    End,
    /// A row (mouse click).
    Row(u32),
}

/// What happens to the selection when the focus moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Select {
    /// Selection = the focused entry (plain arrows, click).
    Replace,
    /// Selection = rows from the anchor to the focus (Shift).
    Extend,
    /// Toggle the target, keep the rest (Ctrl+click).
    Toggle,
    /// Only the focus moves (Ctrl+arrows).
    FocusOnly,
}

/// Moving the selection with the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shift {
    /// Above the previous unselected row (Alt+↑).
    Up,
    /// Below the next unselected row (Alt+↓).
    Down,
    /// To the start of the block, then of the previous one (Alt+Shift+↑).
    BlockStart,
    /// To the end of the block, then of the next one (Alt+Shift+↓).
    BlockEnd,
}

impl OrderBook {
    pub fn focus(&self, v: ViewId) -> Option<Key> {
        self.view(v).cursor.focus
    }

    /// Entries of view `v`'s selection in canon order (the focused one when nothing is selected).
    pub fn selection(&self, v: ViewId) -> Vec<Key> {
        let c = &self.view(v).cursor;
        let mut sel: Vec<Key> = c.selected.iter().copied().collect();
        if sel.is_empty() {
            sel.extend(c.focus);
        }
        sel.sort_by_key(|&k| self.seq.pos(k));
        sel
    }

    pub fn selection_len(&self, v: ViewId) -> usize {
        self.view(v).cursor.selected.len()
    }

    /// Row carrying view `v`'s focus mark: the focused entry, or the row above it when it is hidden.
    pub fn focus_row(&self, v: ViewId) -> Option<usize> {
        let f = self.view(v).cursor.focus?;
        let rows = self.seq.visible_len(v);
        if rows == 0 || !self.seq.contains(f) {
            return None;
        }
        Some(match self.seq.row(f, v) {
            Some(r) => r,
            None => self.seq.rows_before(f, v).saturating_sub(1).min(rows - 1),
        })
    }

    pub(super) fn effective_focus(&self, v: ViewId) -> Option<Key> {
        self.focus_row(v).map(|r| self.seq.at_row(r, v))
    }

    /// Repaints rows of `keys` in view `v` (visible ones), or the whole list when there are many.
    fn touch(&mut self, v: ViewId, keys: impl IntoIterator<Item = Key>, p: &mut Patches) {
        let keys: Vec<Key> = keys
            .into_iter()
            .filter(|&k| self.seq.contains(k) && self.seq.is_visible(k, v))
            .collect();
        let rows = self.seq.visible_len(v);
        if keys.len() > BULK && rows > 0 {
            p.0.push(super::Patch::Update {
                at: 0,
                count: rows as u32,
            });
            return;
        }
        let mut rows_of: Vec<usize> = keys.iter().map(|&k| self.seq.rows_before(k, v)).collect();
        rows_of.sort_unstable();
        for r in rows_of {
            p.update(r);
        }
    }

    /// Keeps view `v`'s focus mark in step with where it is drawn (every operation does it before returning).
    pub(super) fn sync_focus_mark(&mut self, v: ViewId, p: &mut Patches) {
        let now = self.effective_focus(v);
        let shown = self.view(v).cursor.shown;
        if now != shown {
            self.view_mut(v).cursor.shown = now;
            self.touch(v, shown.into_iter().chain(now), p);
        }
    }

    fn cursor_change(&mut self, v: ViewId, p: Patches) -> Change {
        let mut out = self.out_one(v);
        *out.p(v) = p;
        self.finish(out)
    }

    // @sat:order.cursor.navigate
    /// Moves view `v`'s focus and updates its selection per [`Select`].
    pub fn navigate(&mut self, v: ViewId, nav: Nav, select: Select) -> Result<Change, OrderError> {
        self.check_view(v)?;
        let mut p = Patches::default();
        let rows = self.seq.visible_len(v);
        if rows == 0 {
            return Ok(self.cursor_change(v, p));
        }
        let cur = self.focus_row(v);
        let last = rows - 1;
        let target = match (nav, cur) {
            (Nav::Row(r), _) => (r as usize).min(last),
            (_, None) => 0,
            (Nav::Up, Some(c)) => c.saturating_sub(1),
            (Nav::Down, Some(c)) => (c + 1).min(last),
            (Nav::PageUp(n), Some(c)) => c.saturating_sub(n.max(1) as usize),
            (Nav::PageDown(n), Some(c)) => (c + n.max(1) as usize).min(last),
            (Nav::Home, _) => 0,
            (Nav::End, _) => last,
        };
        let key = self.seq.at_row(target, v);
        self.view_mut(v).cursor.focus = Some(key);
        match select {
            Select::FocusOnly => {}
            Select::Replace => {
                let c = &mut self.view_mut(v).cursor;
                let old = std::mem::take(&mut c.selected);
                c.selected.insert(key);
                c.anchor = Some(key);
                self.touch(v, old.into_iter().chain([key]), &mut p);
            }
            Select::Toggle => {
                let c = &mut self.view_mut(v).cursor;
                if !c.selected.remove(&key) {
                    c.selected.insert(key);
                }
                c.anchor = Some(key);
                self.touch(v, [key], &mut p);
            }
            Select::Extend => {
                let anchor = match self.view(v).cursor.anchor.filter(|&a| self.seq.contains(a)) {
                    Some(a) => a,
                    None => {
                        self.view_mut(v).cursor.anchor = Some(key);
                        key
                    }
                };
                let a_row = self
                    .seq
                    .row(anchor, v)
                    .unwrap_or_else(|| self.seq.rows_before(anchor, v).min(last));
                let (lo, hi) = (a_row.min(target), a_row.max(target));
                let new: FxHashSet<Key> = self.seq.rows_from(lo, v).take(hi - lo + 1).collect();
                let old = std::mem::replace(&mut self.view_mut(v).cursor.selected, new);
                let changed: Vec<Key> = old
                    .symmetric_difference(&self.view(v).cursor.selected)
                    .copied()
                    .collect();
                self.touch(v, changed, &mut p);
            }
        }
        Ok(self.cursor_change(v, p))
    }

    /// Adds the focused entry to the selection or removes it (Space).
    pub fn toggle_focused(&mut self, v: ViewId) -> Option<Change> {
        let f = self.views.get(v)?.as_ref()?.cursor.focus?;
        let mut p = Patches::default();
        let c = &mut self.view_mut(v).cursor;
        if !c.selected.remove(&f) {
            c.selected.insert(f);
        }
        c.anchor = Some(f);
        self.touch(v, [f], &mut p);
        Some(self.cursor_change(v, p))
    }

    /// Selects every row of view `v`.
    pub fn select_all(&mut self, v: ViewId) -> Result<Change, OrderError> {
        self.check_view(v)?;
        let mut p = Patches::default();
        let all: FxHashSet<Key> = self.seq.rows_from(0, v).collect();
        let old = std::mem::replace(&mut self.view_mut(v).cursor.selected, all);
        let changed: Vec<Key> = old
            .symmetric_difference(&self.view(v).cursor.selected)
            .copied()
            .collect();
        self.touch(v, changed, &mut p);
        Ok(self.cursor_change(v, p))
    }

    pub fn clear_selection(&mut self, v: ViewId) -> Result<Change, OrderError> {
        self.check_view(v)?;
        let mut p = Patches::default();
        let old = std::mem::take(&mut self.view_mut(v).cursor.selected);
        self.touch(v, old, &mut p);
        Ok(self.cursor_change(v, p))
    }

    // @sat:order.cursor.shift
    /// Moves view `v`'s selection (or its focused entry) with the keyboard; undoable like any move. Rows are
    /// those of view `v`; the move itself is a canon operation, so every view gets its patches.
    pub fn shift_selection(&mut self, v: ViewId, how: Shift) -> Result<Option<Change>, OrderError> {
        self.check_view(v)?;
        // entries the manager does not manage (DLC, game files) never move
        let sel: Vec<Key> = self
            .selection(v)
            .into_iter()
            .filter(|&k| self.kind(k) != EntryKind::Foreign)
            .collect();
        if sel.is_empty() {
            return Ok(None);
        }
        let picked: FxHashSet<Key> = sel.iter().copied().collect();
        let len = self.seq.len();
        // A separator moves with its block, so one step for it is one block: by a row it would land inside the
        // neighbouring block and take that block's tail with it.
        let has_sep = sel.iter().any(|&k| self.kind(k) == EntryKind::Separator);
        let how = match how {
            Shift::Up if has_sep => Shift::BlockStart,
            Shift::Down if has_sep => Shift::BlockEnd,
            h => h,
        };
        let before: Option<Key> = match how {
            Shift::Up | Shift::Down => {
                let rows: Vec<usize> = sel.iter().filter_map(|&k| self.seq.row(k, v)).collect();
                let (Some(&lo), Some(&hi)) = (rows.iter().min(), rows.iter().max()) else {
                    return Ok(None);
                };
                let free = |r: &usize| !picked.contains(&self.seq.at_row(*r, v));
                if how == Shift::Up {
                    match (0..lo).rev().find(free) {
                        Some(r) => Some(self.seq.at_row(r, v)),
                        None => return Ok(None),
                    }
                } else {
                    let rows_n = self.seq.visible_len(v);
                    let Some(n) = (hi + 1..rows_n).find(free) else {
                        return Ok(None);
                    };
                    (n + 1..rows_n).find(free).map(|m| self.seq.at_row(m, v))
                }
            }
            Shift::BlockStart => {
                let first = self.seq.pos(sel[0]);
                let start_of = |pos: usize| self.seq.separator_before(pos).map_or(0, |s| self.seq.pos(s) + 1);
                let mut start = if self.kind(sel[0]) == EntryKind::Separator {
                    first
                } else {
                    start_of(first)
                };
                if start >= first {
                    // already at the start: the previous block (for a separator: before the previous separator)
                    match self.seq.separator_before(start.saturating_sub(1)) {
                        Some(s) => {
                            start = self.seq.pos(s);
                            if self.kind(sel[0]) != EntryKind::Separator {
                                start = start_of(start);
                                if start >= first {
                                    return Ok(None);
                                }
                            }
                        }
                        None if start > 0 && self.kind(sel[0]) == EntryKind::Separator => start = 0,
                        None => return Ok(None),
                    }
                }
                (start < len).then(|| self.seq.at(start))
            }
            Shift::BlockEnd => {
                // the last position the move takes (a separator brings its block)
                let last = sel
                    .iter()
                    .map(|&k| {
                        let p = self.seq.pos(k);
                        if self.kind(k) == EntryKind::Separator {
                            self.seq.block_end(p + 1) - 1
                        } else {
                            p
                        }
                    })
                    .max()
                    .expect("selection is not empty");
                let mut end = self.seq.block_end(last + 1);
                if end == last + 1 {
                    if end >= len {
                        return Ok(None);
                    }
                    end = self.seq.block_end(end + 1);
                }
                (end < len).then(|| self.seq.at(end))
            }
        };
        self.move_keys(&sel, before)
    }

    /// ← / →: collapses or expands the focused separator or group in view `v`; ← on a mod moves the focus to
    /// its group's first member or its separator.
    pub fn fold_focused(&mut self, v: ViewId, expand: bool) -> Result<Option<Change>, OrderError> {
        self.check_view(v)?;
        let Some(f) = self.effective_focus(v) else {
            return Ok(None);
        };
        if self.kind(f) == EntryKind::Separator {
            return self.set_collapsed(v, f, !expand);
        }
        if let Some((g, gr)) = self.group(f) {
            let (valid, first) = (gr.valid, gr.first);
            let collapsed = self.group_collapsed(g, v);
            if valid && first == f && collapsed == expand {
                return self.set_group_collapsed(v, g, !expand);
            }
            // a filter may hide the first member; then ← goes on to the separator
            if valid
                && !collapsed
                && !expand
                && first != f
                && let Some(r) = self.seq.row(first, v)
            {
                return Ok(Some(self.navigate(v, Nav::Row(r as u32), Select::Replace)?));
            }
        }
        if !expand
            && let Some(s) = self.owner(f)
            && let Some(r) = self.seq.row(s, v)
        {
            return Ok(Some(self.navigate(v, Nav::Row(r as u32), Select::Replace)?));
        }
        Ok(None)
    }

    /// Type-ahead: the next row of view `v` after the focus whose name starts with `prefix` (case-insensitive).
    pub fn jump_to(&mut self, v: ViewId, prefix: &str) -> Option<Change> {
        self.check_view(v).ok()?;
        let rows = self.seq.visible_len(v);
        if rows == 0 || prefix.is_empty() {
            return None;
        }
        let want = prefix.to_lowercase();
        let start = self.focus_row(v).map_or(0, |r| r + 1);
        let hit = self
            .seq
            .rows_from(start, v)
            .chain(self.seq.rows_from(0, v).take(start))
            .find(|&k| self.items[k as usize].folded.starts_with(&want))?;
        let r = self.seq.row(hit, v)?;
        self.navigate(v, Nav::Row(r as u32), Select::Replace).ok()
    }

    // @sat:order.cursor.focus-entry
    /// Puts view `v`'s focus on `key` (a link from another window: "go to this mod"). A mod hidden in a collapsed
    /// separator block is revealed by expanding that block; a mod hidden by the filter or a collapsed group is
    /// not reachable and returns `None`.
    pub fn focus_entry(&mut self, v: ViewId, key: Key) -> Option<Change> {
        self.check_view(v).ok()?;
        if !self.seq.contains(key) {
            return None;
        }
        let mut opened = None;
        if self.seq.row(key, v).is_none()
            && let Some(sep) = self.seq.separator_before(self.seq.pos(key))
            && self.view(v).collapsed.contains(&sep)
        {
            opened = self.set_collapsed(v, sep, false).ok().flatten();
        }
        let r = self.seq.row(key, v)?;
        let c = self.navigate(v, Nav::Row(r as u32), Select::Replace).ok()?;
        let Some(mut first) = opened else { return Some(c) };
        // one change: the expansion's patches, then the cursor's
        for later in c.views {
            match first.views.iter_mut().find(|x| x.view == later.view) {
                Some(x) => {
                    x.patches.extend(later.patches);
                    x.len = later.len;
                }
                None => first.views.push(later),
            }
        }
        first.revision = c.revision;
        first.truth.extend(c.truth);
        Some(first)
    }

    /// Entries left the list: they leave every selection; a removed focus goes to the entry now at `pos`.
    pub(super) fn cursor_forget(&mut self, keys: &[Key], pos: usize, out: &mut Out) {
        let len = self.seq.len();
        let next = (len > 0).then(|| self.seq.at(pos.min(len - 1)));
        for i in 0..out.views.len() {
            let v = out.views[i].0;
            let c = &mut self.view_mut(v).cursor;
            for k in keys {
                c.selected.remove(k);
            }
            if c.focus.is_some_and(|f| keys.contains(&f)) {
                c.focus = next;
            }
            if c.anchor.is_some_and(|a| keys.contains(&a)) {
                c.anchor = c.focus;
            }
            // the focus mark itself is synchronised by `finish`
        }
    }
}
