//! Keyboard cursor of the list: **focus** (one entry, where keys act), **anchor** (where a Shift range starts) and @sat:order.cursor
//! **selection** (a set). All three are entry keys, not row numbers, so they survive moves, filters and folds;
//! every change returns row patches like any other operation (ADR 0008).
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

use super::book::{Change, OrderBook, OrderError, Patches};
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
    pub fn focus(&self) -> Option<Key> {
        self.cursor.focus
    }

    /// Entries of the selection in canon order (the focused one when nothing is selected).
    pub fn selection(&self) -> Vec<Key> {
        let mut v: Vec<Key> = self.cursor.selected.iter().copied().collect();
        if v.is_empty() {
            v.extend(self.cursor.focus);
        }
        v.sort_by_key(|&k| self.seq.pos(k));
        v
    }

    pub fn selection_len(&self) -> usize {
        self.cursor.selected.len()
    }

    /// Row carrying the focus mark: the focused entry, or the visible row above it when it is hidden.
    pub fn focus_row(&self) -> Option<usize> {
        let f = self.cursor.focus?;
        let rows = self.seq.visible_len();
        if rows == 0 || !self.seq.contains(f) {
            return None;
        }
        Some(match self.seq.row(f) {
            Some(r) => r,
            None => self.seq.rows_before(f).saturating_sub(1).min(rows - 1),
        })
    }

    pub(super) fn effective_focus(&self) -> Option<Key> {
        self.focus_row().map(|r| self.seq.at_row(r))
    }

    /// Repaints rows of `keys` (visible ones), or the whole list when there are many.
    fn touch(&mut self, keys: impl IntoIterator<Item = Key>, p: &mut Patches) {
        let keys: Vec<Key> = keys
            .into_iter()
            .filter(|&k| self.seq.contains(k) && self.seq.is_visible(k))
            .collect();
        let rows = self.seq.visible_len();
        if keys.len() > BULK && rows > 0 {
            p.0.push(super::Patch::Update {
                at: 0,
                count: rows as u32,
            });
            return;
        }
        self.reweigh(keys, |_| true, p);
    }

    /// Keeps the focus mark in step with where it is drawn (called by every operation before it returns).
    pub(super) fn sync_focus_mark(&mut self, p: &mut Patches) {
        let now = self.effective_focus();
        if now != self.cursor.shown {
            let old = self.cursor.shown;
            self.cursor.shown = now;
            self.touch(old.into_iter().chain(now), p);
        }
    }

    // @sat:order.cursor.navigate
    pub fn navigate(&mut self, nav: Nav, select: Select) -> Change {
        let mut p = Patches::default();
        let rows = self.seq.visible_len();
        if rows == 0 {
            return self.finish(p);
        }
        let cur = self.focus_row();
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
        let key = self.seq.at_row(target);
        self.cursor.focus = Some(key);
        match select {
            Select::FocusOnly => {}
            Select::Replace => {
                let old = std::mem::take(&mut self.cursor.selected);
                self.cursor.selected.insert(key);
                self.cursor.anchor = Some(key);
                self.touch(old.into_iter().chain([key]), &mut p);
            }
            Select::Toggle => {
                if !self.cursor.selected.remove(&key) {
                    self.cursor.selected.insert(key);
                }
                self.cursor.anchor = Some(key);
                self.touch([key], &mut p);
            }
            Select::Extend => {
                let anchor = match self.cursor.anchor.filter(|&a| self.seq.contains(a)) {
                    Some(a) => a,
                    None => {
                        self.cursor.anchor = Some(key);
                        key
                    }
                };
                let a_row = self
                    .seq
                    .row(anchor)
                    .unwrap_or_else(|| self.seq.rows_before(anchor).min(last));
                let (lo, hi) = (a_row.min(target), a_row.max(target));
                let new: FxHashSet<Key> = self.seq.rows_from(lo).take(hi - lo + 1).collect();
                let old = std::mem::replace(&mut self.cursor.selected, new);
                let changed: Vec<Key> = old.symmetric_difference(&self.cursor.selected).copied().collect();
                self.touch(changed, &mut p);
            }
        }
        self.finish(p)
    }

    /// Adds the focused entry to the selection or removes it (Space).
    pub fn toggle_focused(&mut self) -> Option<Change> {
        let f = self.cursor.focus?;
        let mut p = Patches::default();
        if !self.cursor.selected.remove(&f) {
            self.cursor.selected.insert(f);
        }
        self.cursor.anchor = Some(f);
        self.touch([f], &mut p);
        Some(self.finish(p))
    }

    /// Selects every visible row.
    pub fn select_all(&mut self) -> Change {
        let mut p = Patches::default();
        let all: FxHashSet<Key> = self.seq.rows_from(0).collect();
        let old = std::mem::replace(&mut self.cursor.selected, all);
        let changed: Vec<Key> = old.symmetric_difference(&self.cursor.selected).copied().collect();
        self.touch(changed, &mut p);
        self.finish(p)
    }

    pub fn clear_selection(&mut self) -> Change {
        let mut p = Patches::default();
        let old = std::mem::take(&mut self.cursor.selected);
        self.touch(old, &mut p);
        self.finish(p)
    }

    // @sat:order.cursor.shift
    /// Moves the selection (or the focused entry) with the keyboard; undoable like any move.
    pub fn shift_selection(&mut self, how: Shift) -> Result<Option<Change>, OrderError> {
        // entries the manager does not manage (DLC, game files) never move
        let sel: Vec<Key> = self
            .selection()
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
                let rows: Vec<usize> = sel.iter().filter_map(|&k| self.seq.row(k)).collect();
                let (Some(&lo), Some(&hi)) = (rows.iter().min(), rows.iter().max()) else {
                    return Ok(None);
                };
                let free = |r: &usize| !picked.contains(&self.seq.at_row(*r));
                if how == Shift::Up {
                    match (0..lo).rev().find(free) {
                        Some(r) => Some(self.seq.at_row(r)),
                        None => return Ok(None),
                    }
                } else {
                    let rows_n = self.seq.visible_len();
                    let Some(n) = (hi + 1..rows_n).find(free) else {
                        return Ok(None);
                    };
                    (n + 1..rows_n).find(free).map(|m| self.seq.at_row(m))
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

    /// ← / →: collapses or expands the focused separator or group; ← on a mod moves the focus to its separator.
    pub fn fold_focused(&mut self, expand: bool) -> Result<Option<Change>, OrderError> {
        let Some(f) = self.effective_focus() else {
            return Ok(None);
        };
        if self.kind(f) == EntryKind::Separator {
            return self.set_collapsed(f, !expand);
        }
        if let Some((g, gr)) = self.group(f) {
            if gr.valid && gr.first == f && gr.collapsed == expand {
                return self.set_group_collapsed(g, !expand);
            }
            // a filter may hide the first member; then ← goes on to the separator
            if gr.valid
                && !gr.collapsed
                && !expand
                && gr.first != f
                && let Some(r) = self.seq.row(gr.first)
            {
                return Ok(Some(self.navigate(Nav::Row(r as u32), Select::Replace)));
            }
        }
        if !expand
            && let Some(s) = self.owner(f)
            && let Some(r) = self.seq.row(s)
        {
            return Ok(Some(self.navigate(Nav::Row(r as u32), Select::Replace)));
        }
        Ok(None)
    }

    /// Type-ahead: the next visible row after the focus whose name starts with `prefix` (case-insensitive).
    pub fn jump_to(&mut self, prefix: &str) -> Option<Change> {
        let rows = self.seq.visible_len();
        if rows == 0 || prefix.is_empty() {
            return None;
        }
        let want = prefix.to_lowercase();
        let start = self.focus_row().map_or(0, |r| r + 1);
        let hit = self
            .seq
            .rows_from(start)
            .chain(self.seq.rows_from(0).take(start))
            .find(|&k| self.items[k as usize].folded.starts_with(&want))?;
        let r = self.seq.row(hit)?;
        Some(self.navigate(Nav::Row(r as u32), Select::Replace))
    }

    /// Entries left the list: they leave the selection; a removed focus goes to the entry now at `pos`.
    pub(super) fn cursor_forget(&mut self, keys: &[Key], pos: usize, p: &mut Patches) {
        for k in keys {
            self.cursor.selected.remove(k);
        }
        let len = self.seq.len();
        let next = (len > 0).then(|| self.seq.at(pos.min(len - 1)));
        if self.cursor.focus.is_some_and(|f| keys.contains(&f)) {
            self.cursor.focus = next;
            self.touch(next, p);
        }
        if self.cursor.anchor.is_some_and(|a| keys.contains(&a)) {
            self.cursor.anchor = self.cursor.focus;
        }
    }
}
