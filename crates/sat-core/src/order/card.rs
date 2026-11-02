//! The mod container on one line: where its indicator, badges, name and counters go (ADR 0016). @sat:order.card
//!
//! Owns: the single-line container — indicator on the left, then the badges, the name, the counters on the right
//! — its minimum width, and which parts stay whole when the container is narrow: the indicator and the badges
//! always, the counters as a group that grows to the left, the name is cut (the window draws an ellipsis).
//! Not here: where containers go in the view (`@sat:order.flow`), text measuring (the window's: the core counts
//! characters of counters only, digits are of one width).

use serde::{Deserialize, Serialize};

/// Sizes the window gives (pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CardMetrics {
    pub pad: u32,
    pub indicator: u32,
    pub badge: u32,
    /// Width of one counter character (digits are tabular).
    pub digit: u32,
    /// Space between parts.
    pub gap: u32,
    /// The name is never narrower than this (a few letters and an ellipsis).
    pub min_name: u32,
}

impl Default for CardMetrics {
    fn default() -> Self {
        Self {
            pad: 6,
            indicator: 14,
            badge: 16,
            digit: 7,
            gap: 6,
            min_name: 48,
        }
    }
}

/// A part's place inside the container (x from its left edge, and width).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub x: u32,
    pub width: u32,
}

/// Where the parts of a single-line container go.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SingleLine {
    pub indicator: Span,
    pub badges: Span,
    pub name: Span,
    pub counters: Span,
    /// The name has less room than it needs at its natural width (the window cuts it).
    pub name_cut: bool,
}

impl CardMetrics {
    fn fixed(&self, badges: u32, counter_chars: u32) -> u32 {
        let b = badges * self.badge;
        let c = counter_chars * self.digit;
        self.pad * 2
            + self.indicator
            + if b > 0 { self.gap + b } else { 0 }
            + self.gap
            + if c > 0 { self.gap + c } else { 0 }
    }

    // @sat:order.card.min-width
    /// The narrowest a single-line container may be: everything but the name whole, the name at its minimum.
    pub fn min_width(&self, badges: u32, counter_chars: u32) -> u32 {
        self.fixed(badges, counter_chars) + self.min_name
    }

    // @sat:order.card.single-line
    /// Lays out a single-line container of `width` with `badges` badges, counters of `counter_chars`
    /// characters, and a name that wants `name_width` (0: unknown — it takes the room left). A container narrower
    /// than [`CardMetrics::min_width`] keeps the indicator and the badges; the counters are pushed out to the
    /// right before the name loses its minimum.
    pub fn single_line(&self, width: u32, badges: u32, counter_chars: u32, name_width: u32) -> SingleLine {
        let ind = Span {
            x: self.pad,
            width: self.indicator,
        };
        let bw = badges * self.badge;
        let bx = ind.x + ind.width + if bw > 0 { self.gap } else { 0 };
        let badges = Span { x: bx, width: bw };
        let name_x = bx + bw + self.gap;
        let cw = counter_chars * self.digit;
        // the counters hold the right edge and grow to the left
        let right = width.saturating_sub(self.pad);
        let floor = if cw > 0 {
            name_x + self.min_name + self.gap
        } else {
            right
        };
        let counters_x = right.saturating_sub(cw).max(floor.min(right));
        let counters = Span {
            x: counters_x,
            width: right.saturating_sub(counters_x).min(cw),
        };
        let name_end = if cw > 0 {
            counters_x.saturating_sub(self.gap)
        } else {
            right
        };
        let room = name_end.saturating_sub(name_x);
        SingleLine {
            indicator: ind,
            badges,
            name: Span {
                x: name_x,
                width: room,
            },
            counters,
            name_cut: name_width > room,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_gives_way_and_the_rest_stays_whole() {
        let m = CardMetrics::default();
        // wide: indicator, 2 badges, name, counters at the right edge
        let l = m.single_line(400, 2, 8, 120);
        assert_eq!(l.indicator, Span { x: 6, width: 14 });
        assert_eq!(l.badges, Span { x: 26, width: 32 });
        assert_eq!(l.name.x, 64);
        assert_eq!(l.counters.x + l.counters.width, 394);
        assert!(!l.name_cut && l.name.x + l.name.width + m.gap <= l.counters.x);
        // at the minimum width the name keeps its minimum
        let w = m.min_width(2, 8);
        let l = m.single_line(w, 2, 8, 300);
        assert_eq!(l.name.width, m.min_name);
        assert_eq!(l.counters.width, 8 * m.digit);
        assert!(l.name_cut);
        // narrower still: the counters are pushed out before the name goes below its minimum
        let l = m.single_line(w - 20, 2, 8, 300);
        assert_eq!(l.name.width, m.min_name);
        assert!(l.counters.width < 8 * m.digit);
        assert_eq!(l.badges.width, 32, "badges never shrink");
        // no badges, no counters: the name takes the line
        let l = m.single_line(200, 0, 0, 0);
        assert_eq!((l.name.x, l.name.width), (26, 200 - 6 - 26));
    }
}
