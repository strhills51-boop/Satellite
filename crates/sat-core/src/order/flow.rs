//! Flow layout of a view: cards in lines inside each separator block (ADR 0012 §2). @sat:order.flow
//!
//! Owns: the block index of a view under a viewport (each block's first row, row count, y and height), the line
//! layout of a block, the window of lines at a scroll position, and the y of a row (to keep the focus visible).
//! Not here: card styles as data (`@sat:order.decor.style`), rows and patches (`@sat:order.book`), drawing.
//!
//! Cost model (law 11 of ADR 0010): the index is built from separators only — `O(separators · log n)` — because a
//! block without styled entries has a closed-form layout (`per_line` cards of the default width, lines of the
//! default height). Only blocks that contain styled entries are laid out entry by entry, and only those. A window
//! query lays out just the blocks it crosses.

use serde::{Deserialize, Serialize};

use super::book::{OrderBook, ViewId};
use super::seq::Key;

/// Viewport and defaults of a view's flow layout (pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowParams {
    pub width: u32,
    pub card_width: u32,
    pub card_height: u32,
    pub header_height: u32,
    pub gap: u32,
    /// Spare room at the end of every block, before the next separator (ADR 0016): a block's height is its natural
    /// height plus the reserve, rounded up to a multiple of it — a card that grows a little mostly moves nothing
    /// below (only crossing a multiple does). 0 — no spare room.
    #[serde(default)]
    pub reserve: u32,
    /// Bricks (ADR 0020 §6): a card is as wide as its name — `None` keeps the grid of equal cards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natural: Option<Natural>,
}

// @sat:order.flow.natural
/// The natural width of a card in the bricks layout: its name decides it (the window measures nothing — an
/// average glyph width is enough, a name that does not fit is cut, ADR 0020 §6). A line is read left to right,
/// lines top to bottom; a full line shares what is left of it among its cards, the last line of a block keeps
/// the natural widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Natural {
    /// Average glyph width of a name, in tenths of a pixel.
    pub glyph10: u32,
    /// Everything besides the name: padding, indicator, room for badges and counters.
    pub chrome: u32,
    /// The common minimum width of every container.
    pub min: u32,
}

impl Natural {
    /// `extra` — what else the container holds besides its name (badges, counters), as the caller measures it.
    fn width(&self, name: &str, extra: u32, max: u32) -> u32 {
        let chars = name.chars().count() as u32;
        (self.chrome + extra + (chars * self.glyph10).div_ceil(10))
            .max(self.min)
            .min(max)
    }
}

/// What a container holds besides its name, by key, in pixels (badges, counters): the caller's measure.
pub type Extra<'a> = &'a dyn Fn(Key) -> u32;

fn no_extra(_: Key) -> u32 {
    0
}

impl FlowParams {
    fn per_line(&self) -> u32 {
        ((self.width + self.gap) / (self.card_width + self.gap)).max(1)
    }
    /// Actual width of a default card: the line is shared evenly (cards fill the width like a grid).
    fn default_card_width(&self) -> u32 {
        let n = self.per_line();
        (self.width.saturating_sub(self.gap * (n - 1)) / n).max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Block {
    /// The block's separator (`None` for the entries before the first separator).
    sep: Option<Key>,
    /// Header (the separator row) when it is visible in the view.
    header: Option<Key>,
    /// First row after the header.
    first_row: u32,
    /// Rows of the block after the header (visible in the view).
    rows: u32,
    y: u32,
    height: u32,
    styled: bool,
}

// @sat:order.flow.index
/// The block index of one view under given parameters. Built per book revision; queries are `O(log blocks)`.
#[derive(Debug, Clone)]
pub struct FlowIndex {
    pub params: FlowParams,
    pub revision: u64,
    blocks: Vec<Block>,
    pub total_height: u32,
}

/// One placed card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placed {
    pub row: u32,
    pub key: Key,
    pub x: u32,
    pub width: u32,
    pub height: u32,
    /// Offset of the card's top from the line's top: the same for every card of a line (the indicator line, below).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dy: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// One line of the layout: a separator header or a line of cards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Line {
    pub y: u32,
    pub height: u32,
    pub header: bool,
    pub cards: Vec<Placed>,
}

impl OrderBook {
    /// Builds the block index of view `v`.
    pub fn flow_index(&self, v: ViewId, params: FlowParams) -> FlowIndex {
        self.flow_index_in(v, params, None)
    }

    /// [`flow_index`](Self::flow_index) with what each container holds besides its name (bricks).
    pub fn flow_index_by(&self, v: ViewId, params: FlowParams, extra: Extra<'_>) -> FlowIndex {
        self.flow_index_with(v, params, None, extra)
    }

    // @sat:order.flow.scope
    /// Builds the block index of view `v` limited to the blocks of the given separators (the monolith's classic
    /// view shows the current separator, or the selected ones stacked). Rows keep their view numbers.
    pub fn flow_index_in(&self, v: ViewId, params: FlowParams, scope: Option<&[Key]>) -> FlowIndex {
        self.flow_index_with(v, params, scope, &no_extra)
    }

    fn flow_index_with(
        &self,
        v: ViewId,
        params: FlowParams,
        scope: Option<&[Key]>,
        extra: Extra<'_>,
    ) -> FlowIndex {
        let rows_total = self.row_count(v) as u32;
        let seps = self.seq.separator_count();
        let mut blocks = Vec::with_capacity(seps + 1);
        // blocks with styled entries (visible in this view)
        let mut styled_seps: rustc_hash::FxHashSet<Option<Key>> = Default::default();
        for &k in &self.styled {
            if self.seq.contains(k) && self.seq.is_visible(k, v) {
                styled_seps.insert(self.separator_of(k));
            }
        }
        let first_sep_row = if seps > 0 {
            self.seq.rows_before(self.seq.nth_separator(0).0, v) as u32
        } else {
            rows_total
        };
        if first_sep_row > 0 {
            blocks.push(Block {
                sep: None,
                header: None,
                first_row: 0,
                rows: first_sep_row,
                y: 0,
                height: 0,
                styled: styled_seps.contains(&None),
            });
        }
        for n in 0..seps {
            let (sep, _) = self.seq.nth_separator(n as u32);
            let row = self.seq.rows_before(sep, v) as u32;
            let end = if n + 1 < seps {
                self.seq.rows_before(self.seq.nth_separator(n as u32 + 1).0, v) as u32
            } else {
                rows_total
            };
            let visible = self.seq.is_visible(sep, v);
            blocks.push(Block {
                sep: Some(sep),
                header: visible.then_some(sep),
                first_row: row + u32::from(visible),
                rows: end.saturating_sub(row + u32::from(visible)),
                y: 0,
                height: 0,
                styled: styled_seps.contains(&Some(sep)),
            });
        }
        if let Some(scope) = scope {
            blocks.retain(|b| b.sep.is_some_and(|s| scope.contains(&s)));
        }
        let mut y = 0;
        for b in &mut blocks {
            b.y = y;
            let natural = if b.styled || params.natural.is_some() {
                self.block_lines(v, b, &params, 0, extra)
                    .iter()
                    .map(|l| l.height)
                    .sum()
            } else {
                plain_height(b, &params)
            };
            b.height = with_reserve(natural, params.reserve);
            y += b.height;
        }
        FlowIndex {
            params,
            revision: self.revision(),
            blocks,
            total_height: y,
        }
    }

    /// Lines of view `v` that intersect `[y, y + height)`.
    pub fn flow_lines(&self, v: ViewId, idx: &FlowIndex, y: u32, height: u32) -> Vec<Line> {
        self.flow_lines_by(v, idx, y, height, &no_extra)
    }

    /// [`flow_lines`](Self::flow_lines) with the same `extra` the index was built with.
    pub fn flow_lines_by(
        &self,
        v: ViewId,
        idx: &FlowIndex,
        y: u32,
        height: u32,
        extra: Extra<'_>,
    ) -> Vec<Line> {
        let end = y.saturating_add(height);
        let mut out = Vec::new();
        let start = idx.blocks.partition_point(|b| b.y + b.height <= y);
        for b in &idx.blocks[start..] {
            if b.y >= end {
                break;
            }
            if b.styled || idx.params.natural.is_some() {
                out.extend(
                    self.block_lines(v, b, &idx.params, b.y, extra)
                        .into_iter()
                        .filter(|l| l.y + l.height > y && l.y < end),
                );
            } else {
                self.plain_lines(v, b, &idx.params, y, end, &mut out);
            }
        }
        out
    }

    // @sat:order.flow.row-y
    /// The line (y, height) holding row `row` of view `v`, to scroll the focus into view.
    pub fn flow_row_y(&self, v: ViewId, idx: &FlowIndex, row: u32) -> Option<(u32, u32)> {
        self.flow_row_y_by(v, idx, row, &no_extra)
    }

    /// [`flow_row_y`](Self::flow_row_y) with the same `extra` the index was built with.
    pub fn flow_row_y_by(
        &self,
        v: ViewId,
        idx: &FlowIndex,
        row: u32,
        extra: Extra<'_>,
    ) -> Option<(u32, u32)> {
        let i = idx.blocks.partition_point(|b| b.first_row + b.rows <= row);
        let b = idx.blocks.get(i)?;
        let p = &idx.params;
        if b.header.is_some() && row + 1 == b.first_row {
            return Some((b.y, p.header_height));
        }
        if row < b.first_row {
            return None; // outside the scope
        }
        if b.styled || p.natural.is_some() {
            return self
                .block_lines(v, b, p, b.y, extra)
                .into_iter()
                .find(|l| l.cards.iter().any(|c| c.row == row))
                .map(|l| (l.y, l.height));
        }
        let head = if b.header.is_some() { p.header_height } else { 0 };
        let line = (row - b.first_row) / p.per_line();
        Some((b.y + head + line * (p.card_height + p.gap), p.card_height + p.gap))
    }

    fn plain_lines(&self, v: ViewId, b: &Block, p: &FlowParams, y: u32, end: u32, out: &mut Vec<Line>) {
        let head = if b.header.is_some() { p.header_height } else { 0 };
        if let Some(h) = b.header
            && b.y + head > y
            && b.y < end
        {
            out.push(Line {
                y: b.y,
                height: head,
                header: true,
                cards: vec![Placed {
                    row: b.first_row - 1,
                    key: h,
                    x: 0,
                    width: p.width,
                    height: head,
                    dy: 0,
                }],
            });
        }
        if b.rows == 0 {
            return;
        }
        let per = p.per_line();
        let lh = p.card_height + p.gap;
        let lines = b.rows.div_ceil(per);
        let top = b.y + head;
        let from = y.saturating_sub(top) / lh;
        let to = (end.saturating_sub(top)).div_ceil(lh).min(lines);
        let cw = p.default_card_width();
        for line in from..to {
            let first = b.first_row + line * per;
            let count = per.min(b.first_row + b.rows - first);
            let cards = self
                .seq
                .rows_from(first as usize, v)
                .take(count as usize)
                .enumerate()
                .map(|(i, k)| Placed {
                    row: first + i as u32,
                    key: k,
                    x: i as u32 * (cw + p.gap),
                    width: cw,
                    height: p.card_height,
                    dy: 0,
                })
                .collect();
            out.push(Line {
                y: top + line * lh,
                height: lh,
                header: false,
                cards,
            });
        }
    }

    /// Greedy layout of a block with styled entries: widths, breaks, full width, heights, margins.
    fn block_lines(&self, v: ViewId, b: &Block, p: &FlowParams, y0: u32, extra: Extra<'_>) -> Vec<Line> {
        let mut out = Vec::new();
        let mut y = y0;
        if let Some(h) = b.header {
            out.push(Line {
                y,
                height: p.header_height,
                header: true,
                cards: vec![Placed {
                    row: b.first_row - 1,
                    key: h,
                    x: 0,
                    width: p.width,
                    height: p.header_height,
                    dy: 0,
                }],
            });
            y += p.header_height;
        }
        let cw = p.default_card_width();
        let mut cur: Vec<Placed> = Vec::new();
        let mut own: Vec<bool> = Vec::new();
        // (top margin, bottom margin) of each card of the current line
        let mut margins: Vec<(u32, u32)> = Vec::new();
        let mut x = 0u32;
        // @sat:order.flow.indicator-line
        // The load order is read by the indicators (ADR 0016): in a line they keep the line of the majority. The
        // cards' top margins vote; every card of the line takes the winning offset, so a minority card is pulled to
        // the majority instead of breaking the reading line.
        // in bricks, cards with no width of their own share what is left of a full line (`stretch`)
        let flush = |cur: &mut Vec<Placed>,
                     margins: &mut Vec<(u32, u32)>,
                     own: &mut Vec<bool>,
                     stretch: bool,
                     x: &mut u32,
                     y: &mut u32,
                     out: &mut Vec<Line>| {
            if stretch && p.natural.is_some() && !cur.is_empty() {
                let used = cur.last().map_or(0, |c| c.x + c.width);
                let free = p.width.saturating_sub(used);
                let n = own.iter().filter(|o| !**o).count() as u32;
                if free > 0 && n > 0 {
                    let (each, mut rest) = (free / n, free % n);
                    let mut shift = 0;
                    for (c, o) in cur.iter_mut().zip(own.iter()) {
                        c.x += shift;
                        if !*o {
                            let add = each + u32::from(rest > 0);
                            rest = rest.saturating_sub(1);
                            c.width += add;
                            shift += add;
                        }
                    }
                }
            }
            own.clear();
            if !cur.is_empty() {
                let dy = majority(margins.iter().map(|m| m.0));
                let body = cur
                    .iter()
                    .zip(margins.iter())
                    .map(|(c, m)| c.height + m.1)
                    .max()
                    .unwrap_or(0);
                for c in cur.iter_mut() {
                    c.dy = dy;
                }
                let h = dy + body + p.gap;
                out.push(Line {
                    y: *y,
                    height: h,
                    header: false,
                    cards: std::mem::take(cur),
                });
                margins.clear();
                *y += h;
            }
            *x = 0;
        };
        for (i, k) in self
            .seq
            .rows_from(b.first_row as usize, v)
            .take(b.rows as usize)
            .enumerate()
        {
            let st = self.style(k);
            let full = st.is_some_and(|s| s.full_width);
            let w = if full {
                p.width
            } else {
                match (st.and_then(|s| s.width), p.natural) {
                    (Some(w), _) => u32::from(w).min(p.width),
                    (None, Some(n)) => n.width(self.name(k), extra(k), p.width),
                    (None, None) => cw,
                }
            };
            let h = st.and_then(|s| s.height).map_or(p.card_height, u32::from);
            let m = st.map_or((0, 0), |s| {
                (
                    u32::from(s.margin_top.unwrap_or(0)),
                    u32::from(s.margin_bottom.unwrap_or(0)),
                )
            });
            let break_before = st.is_some_and(|s| s.break_before) || full;
            if break_before || (x > 0 && x + w > p.width) {
                // a line ended by the width is full: it is stretched; one ended by a break keeps its widths
                let stretch = !break_before;
                flush(
                    &mut cur,
                    &mut margins,
                    &mut own,
                    stretch,
                    &mut x,
                    &mut y,
                    &mut out,
                );
            }
            cur.push(Placed {
                row: b.first_row + i as u32,
                key: k,
                x,
                width: w,
                height: h,
                dy: 0,
            });
            margins.push(m);
            own.push(full || st.and_then(|s| s.width).is_some());
            x += w + p.gap;
            if full || st.is_some_and(|s| s.break_after) {
                flush(&mut cur, &mut margins, &mut own, false, &mut x, &mut y, &mut out);
            }
        }
        flush(&mut cur, &mut margins, &mut own, false, &mut x, &mut y, &mut out);
        out
    }
}

/// The most common value (ties: the smallest); 0 for none.
fn majority(values: impl Iterator<Item = u32>) -> u32 {
    let mut count: Vec<(u32, u32)> = Vec::new();
    for v in values {
        match count.iter_mut().find(|c| c.0 == v) {
            Some(c) => c.1 += 1,
            None => count.push((v, 1)),
        }
    }
    count
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .map_or(0, |c| c.0)
}

// @sat:order.flow.reserve
/// A block's height with its spare room: `natural + reserve`, rounded up to a multiple of `reserve`.
fn with_reserve(natural: u32, reserve: u32) -> u32 {
    if reserve == 0 {
        natural
    } else {
        (natural + reserve).div_ceil(reserve) * reserve
    }
}

fn plain_height(b: &Block, p: &FlowParams) -> u32 {
    let head = if b.header.is_some() { p.header_height } else { 0 };
    head + b.rows.div_ceil(p.per_line()) * (p.card_height + p.gap)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::{Entry, EntryKind, Modlist};
    use crate::order::MAIN;
    use crate::order::decor::CardStyle;

    fn book(blocks: &[usize]) -> OrderBook {
        let mut entries = Vec::new();
        for (i, &n) in blocks.iter().enumerate() {
            entries.push(Entry {
                name: format!("S{i}_separator"),
                kind: EntryKind::Separator,
                enabled: false,
            });
            for j in 0..n {
                entries.push(Entry {
                    name: format!("M{i}_{j}"),
                    kind: EntryKind::Mod,
                    enabled: true,
                });
            }
        }
        OrderBook::from_modlist(&Modlist::new(entries))
    }

    const P: FlowParams = FlowParams {
        width: 1000,
        card_width: 240,
        card_height: 40,
        header_height: 30,
        gap: 4,
        reserve: 0,
        natural: None,
    };

    /// Every visible row appears exactly once, lines do not overlap and cover the whole height.
    fn check(b: &OrderBook, idx: &FlowIndex) {
        let lines = b.flow_lines(MAIN, idx, 0, idx.total_height);
        let mut rows: Vec<u32> = lines.iter().flat_map(|l| l.cards.iter().map(|c| c.row)).collect();
        rows.sort_unstable();
        assert_eq!(rows, (0..b.row_count(MAIN) as u32).collect::<Vec<_>>());
        let mut y = 0;
        for l in &lines {
            assert_eq!(l.y, y, "lines are contiguous");
            y += l.height;
            let right = l.cards.iter().map(|c| c.x + c.width).max().unwrap_or(0);
            assert!(right <= P.width, "line fits the width");
        }
        assert_eq!(y, idx.total_height);
        for r in 0..b.row_count(MAIN) as u32 {
            let (ly, _) = b.flow_row_y(MAIN, idx, r).expect("row placed");
            let line = lines.iter().find(|l| l.y == ly).expect("a line at that y");
            assert!(
                line.cards.iter().any(|c| c.row == r),
                "row {r} is on the line flow_row_y names"
            );
        }
    }

    #[test]
    fn plain_blocks_have_a_closed_form() {
        let b = book(&[0, 5, 9, 1]);
        let idx = b.flow_index(MAIN, P);
        // 4 cards of 240 + 3 gaps fit in 1000 → per line 4
        assert_eq!(idx.total_height, 4 * 30 + 44 * (2 + 3 + 1));
        check(&b, &idx);
        // a window in the middle returns only the lines it crosses
        let mid = b.flow_lines(MAIN, &idx, 100, 50);
        assert!(!mid.is_empty() && mid.len() <= 3);
    }

    #[test]
    fn styles_change_only_their_blocks() {
        let mut b = book(&[6, 6, 6]);
        let plain = b.flow_index(MAIN, P).blocks;
        let k = b.key_of("M1_2").unwrap();
        b.set_style(
            &[k],
            Some(CardStyle {
                full_width: true,
                height: Some(80),
                ..CardStyle::default()
            }),
        )
        .unwrap();
        let k2 = b.key_of("M1_4").unwrap();
        b.set_style(
            &[k2],
            Some(CardStyle {
                width: Some(600),
                margin_top: Some(6),
                break_after: true,
                ..CardStyle::default()
            }),
        )
        .unwrap();
        let idx = b.flow_index(MAIN, P);
        assert_eq!(idx.blocks[0].height, plain[0].height, "block 0 untouched");
        assert!(idx.blocks[1].styled && idx.blocks[1].height > plain[1].height);
        assert_eq!(
            idx.blocks[2].height, plain[2].height,
            "block 2 keeps its height, only its y moves"
        );
        check(&b, &idx);
        let lines = b.flow_lines(MAIN, &idx, idx.blocks[1].y, idx.blocks[1].height);
        let full = lines.iter().find(|l| l.cards.iter().any(|c| c.key == k)).unwrap();
        assert_eq!(
            (full.cards.len(), full.cards[0].width, full.height),
            (1, P.width, 80 + P.gap)
        );
    }

    #[test]
    fn collapsed_and_filtered_blocks() {
        let mut b = book(&[3, 7, 2]);
        let s1 = b.key_of("S1_separator").unwrap();
        b.set_collapsed(MAIN, s1, true).unwrap();
        let idx = b.flow_index(MAIN, P);
        assert_eq!(idx.blocks[1].rows, 0);
        check(&b, &idx);
        b.set_filter(
            MAIN,
            crate::order::Filter {
                text: "m2_".into(),
                ..Default::default()
            },
        )
        .unwrap();
        let idx = b.flow_index(MAIN, P);
        check(&b, &idx);
    }

    #[test]
    fn a_million_entries_index_is_cheap() {
        let b = book(&vec![100; 10_000]);
        let t = std::time::Instant::now();
        let idx = b.flow_index(MAIN, P);
        let built = t.elapsed();
        let t = std::time::Instant::now();
        let lines = b.flow_lines(MAIN, &idx, idx.total_height / 2, 900);
        let window = t.elapsed();
        assert!(!lines.is_empty());
        // generous bounds for debug builds on CI; release is far below
        assert!(built.as_millis() < 2000, "index {built:?}");
        assert!(window.as_millis() < 50, "window {window:?}");
    }

    #[test]
    fn a_scope_keeps_only_its_blocks() {
        let b = book(&[3, 7, 2]);
        let s1 = b.key_of("S1_separator").unwrap();
        let s2 = b.key_of("S2_separator").unwrap();
        let idx = b.flow_index_in(MAIN, P, Some(&[s1]));
        assert_eq!(idx.total_height, 30 + 44 * 2);
        let lines = b.flow_lines(MAIN, &idx, 0, idx.total_height);
        assert_eq!(lines[0].cards[0].key, s1);
        assert_eq!(
            lines.iter().map(|l| l.cards.len()).sum::<usize>(),
            8,
            "header + 7 mods"
        );
        // rows outside the scope have no place
        assert_eq!(b.flow_row_y(MAIN, &idx, 0), None);
        assert_eq!(b.flow_row_y(MAIN, &idx, 4), Some((0, 30)), "the separator row");
        assert_eq!(b.flow_row_y(MAIN, &idx, 13), None);
        // two separators stack
        let idx = b.flow_index_in(MAIN, P, Some(&[s2, s1]));
        assert_eq!(idx.total_height, 30 + 88 + 30 + 44);
    }

    #[test]
    fn the_indicator_line_follows_the_majority_and_the_reserve_absorbs_growth() {
        let mut b = book(&[6]);
        let keys: Vec<Key> = b.seq.rows_from(1, MAIN).take(3).collect();
        // two cards of the first line want a top margin of 8, one wants 20: all three take 8
        for (k, m) in keys.iter().zip([8u8, 8, 20]) {
            b.set_style(
                &[*k],
                Some(CardStyle {
                    margin_top: Some(m),
                    ..CardStyle::default()
                }),
            )
            .unwrap();
        }
        let idx = b.flow_index(MAIN, P);
        let lines = b.flow_lines(MAIN, &idx, 0, 10_000);
        let first = &lines[1];
        assert!(first.cards.iter().all(|c| c.dy == 8), "{first:?}");
        assert_eq!(first.height, 8 + 40 + 4);
        // spare room: a block's height is a multiple of the reserve, and a small growth stays inside it
        let r = FlowParams { reserve: 64, ..P };
        let before = b.flow_index(MAIN, r);
        assert_eq!(before.total_height % 64, 0);
        b.set_style(
            &[keys[0]],
            Some(CardStyle {
                margin_top: Some(8),
                height: Some(42),
                ..CardStyle::default()
            }),
        )
        .unwrap();
        assert_eq!(b.flow_index(MAIN, r).total_height, before.total_height);
    }

    #[test]
    fn bricks_are_as_wide_as_their_names_and_keep_the_order() {
        let mut entries = vec![Entry {
            name: "S_separator".into(),
            kind: EntryKind::Separator,
            enabled: false,
        }];
        let names = [
            "A",
            "Longer name here",
            "B",
            "Medium one",
            "C",
            "Another long mod name",
            "D",
            "E",
            "F",
        ];
        for n in names {
            entries.push(Entry {
                name: n.into(),
                kind: EntryKind::Mod,
                enabled: true,
            });
        }
        let b = OrderBook::from_modlist(&Modlist::new(entries));
        let p = FlowParams {
            width: 300,
            card_width: 300,
            card_height: 28,
            header_height: 32,
            gap: 6,
            reserve: 0,
            natural: Some(Natural {
                glyph10: 70,
                chrome: 40,
                min: 50,
            }),
        };
        let idx = b.flow_index(MAIN, p);
        let lines = b.flow_lines(MAIN, &idx, 0, idx.total_height);
        let cards: Vec<&Placed> = lines
            .iter()
            .filter(|l| !l.header)
            .flat_map(|l| &l.cards)
            .collect();
        // every mod once, rows increase left to right and top to bottom
        let rows: Vec<u32> = cards.iter().map(|c| c.row).collect();
        assert_eq!(rows, (1..=9).collect::<Vec<_>>());
        let body: Vec<&Line> = lines.iter().filter(|l| !l.header).collect();
        assert!(
            body.len() > 1 && body.len() < 9,
            "several mods per line: {}",
            body.len()
        );
        for (i, l) in body.iter().enumerate() {
            for w in l.cards.windows(2) {
                assert!(
                    w[0].x + w[0].width + p.gap <= w[1].x + 1,
                    "left to right, no overlap"
                );
            }
            assert!(l.cards.iter().all(|c| c.width >= 50), "the common minimum");
            let right = l.cards.last().map_or(0, |c| c.x + c.width);
            if i + 1 < body.len() {
                assert_eq!(right, p.width, "a full line is stretched to the width");
            } else {
                assert!(right <= p.width);
            }
        }
        // the last line keeps the natural widths: a one-letter mod is the minimum
        let last = body.last().unwrap().cards.last().unwrap();
        assert_eq!(last.width, 50);
        // a long name makes a wider card than a short one on the same kind of line
        let long = cards.iter().find(|c| c.row == 6).unwrap().width;
        let short = cards.iter().find(|c| c.row == 7).unwrap().width;
        assert!(long > short, "{long} > {short}");
        // the index height matches the lines
        assert_eq!(idx.total_height, lines.iter().map(|l| l.height).sum::<u32>());
    }
}
