//! Overwrite order advisor: where each mod of a conflict context should sit, from what it is and how it fares. @sat:advisor
//!
//! Owns: the monolith's heuristic score of a mod (`getOverwriteOrderHeuristicScore`, S16 145 070) and the plan
//! (`buildOverwriteOrderAdvisorPlan`, 145 179): strictness, "current truth" blending, which mods move.
//! Not here: the rows of a context and warnings from its paths (`@sat:app.advisor`), applying a plan
//! (`@sat:order.book.place` — mods change places inside their separator, the slots stay).
//!
//! Lower score = earlier in the list (overwritten by later mods). Kept from the monolith:
//! - start 50; `base framework core resource(s) required requirement` −22; `patch compat compatibility fix hotfix
//!   tweak(s) addon extension expansion` +26; `output generated overwrite bodyslide fnis nemesis dyndolod texgen
//!   lodgen synthesis zedit xedit` +42 (whole words of the name);
//! - loses more than twice what it wins (and ≥ 4) −6; wins more than twice what it loses (and ≥ 4) +6;
//! - with the current order as truth: `0.45·score + 0.55·current`, and only a mod that is off by ≥ 26 (aggressive
//!   ≥ 14) makes the plan reorder; without it the plan is the full sort by score.
//!
//! Changed: the monolith clamped the raw global priority to 0..100 as "current" — in a list of 1 000 mods nearly
//! every mod is 100. Here "current" is the mod's place among the context's rows scaled to 0..100. The monolith
//! reordered the whole context and then put mods back by separator; here the plan is per separator from the start
//! (rows carry their separator group), so what is shown is what is applied.

use serde::{Deserialize, Serialize};

/// How bold the plan is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Strictness {
    /// Scores only, nothing moves.
    Inspect,
    /// With the current order as truth: nothing moves (scores and warnings); without: the full sort.
    #[default]
    Safe,
    /// Reorders when a mod is off by ≥ 26.
    Normal,
    /// Reorders when a mod is off by ≥ 14.
    Aggressive,
}

/// One mod of the context, in the current order.
#[derive(Debug, Clone)]
pub struct Row<'a> {
    pub name: &'a str,
    /// Paths of the context it wins / loses.
    pub wins: u32,
    pub loses: u32,
    /// Separator group: rows of one group are contiguous and only move among themselves.
    pub group: u32,
    /// Stays where it is.
    pub locked: bool,
}

/// What the plan says about one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Locked,
    Reorder,
    Keep,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Advice {
    /// The heuristic alone.
    pub heuristic: i32,
    /// The score the plan sorts by (blended with `current` when the current order is truth), one decimal.
    pub score: f64,
    /// Place among the rows, 0..100.
    pub current: u32,
    /// New index among the rows.
    pub to: u32,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    /// Per row, in the input order.
    pub rows: Vec<Advice>,
    /// Row indices in the proposed order.
    pub order: Vec<u32>,
    /// The plan moves something.
    pub reorder: bool,
}

const BASE: &[&str] = &[
    "base",
    "framework",
    "core",
    "resource",
    "resources",
    "required",
    "requirement",
];
const PATCH: &[&str] = &[
    "patch",
    "compat",
    "compatibility",
    "fix",
    "hotfix",
    "tweak",
    "tweaks",
    "addon",
    "extension",
    "expansion",
];
const OUTPUT: &[&str] = &[
    "output",
    "generated",
    "overwrite",
    "bodyslide",
    "fnis",
    "nemesis",
    "dyndolod",
    "texgen",
    "lodgen",
    "synthesis",
    "zedit",
    "xedit",
];

/// The heuristic score of a mod: 50 ± what its name says ± how it fares.
pub fn heuristic(name: &str, wins: u32, loses: u32) -> i32 {
    let lower = name.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |set: &[&str]| words.iter().any(|w| set.contains(w));
    let mut s = 50;
    if has(BASE) {
        s -= 22;
    }
    if has(PATCH) {
        s += 26;
    }
    if has(OUTPUT) {
        s += 42;
    }
    if loses > 2 * wins && loses >= 4 {
        s -= 6;
    }
    if wins > 2 * loses && wins >= 4 {
        s += 6;
    }
    s
}

/// The plan for `rows` (current order).
pub fn plan(rows: &[Row<'_>], strictness: Strictness, current_truth: bool) -> Plan {
    let n = rows.len();
    let current = |i: usize| if n < 2 { 50 } else { (i * 100 / (n - 1)) as u32 };
    let mut out: Vec<Advice> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let h = heuristic(r.name, r.wins, r.loses);
            let score = if current_truth {
                f64::from(h) * 0.45 + f64::from(current(i)) * 0.55
            } else {
                f64::from(h)
            };
            Advice {
                heuristic: h,
                score: (score * 10.0).round() / 10.0,
                current: current(i),
                to: i as u32,
                action: if r.locked { Action::Locked } else { Action::Keep },
            }
        })
        .collect();
    let threshold = match strictness {
        Strictness::Aggressive => 14,
        _ => 26,
    };
    let reorder = match strictness {
        Strictness::Inspect => false,
        _ if !current_truth => true,
        Strictness::Safe => false,
        _ => rows
            .iter()
            .enumerate()
            .any(|(i, r)| !r.locked && (out[i].heuristic - current(i) as i32).abs() >= threshold),
    };
    let mut order: Vec<u32> = (0..n as u32).collect();
    if reorder {
        // per separator group: unlocked rows sort by score among the group's unlocked places
        let mut i = 0;
        while i < n {
            let mut j = i;
            while j < n && rows[j].group == rows[i].group {
                j += 1;
            }
            let places: Vec<usize> = (i..j).filter(|&k| !rows[k].locked).collect();
            let mut movers = places.clone();
            movers.sort_by(|&a, &b| out[a].score.total_cmp(&out[b].score).then(a.cmp(&b)));
            for (&at, &who) in places.iter().zip(&movers) {
                order[at] = who as u32;
            }
            i = j;
        }
    }
    for (to, &who) in order.iter().enumerate() {
        let a = &mut out[who as usize];
        a.to = to as u32;
        if a.action != Action::Locked && to as u32 != who {
            a.action = Action::Reorder;
        }
    }
    Plan {
        reorder: out.iter().any(|a| a.action == Action::Reorder),
        rows: out,
        order,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, group: u32) -> Row<'_> {
        Row {
            name,
            wins: 0,
            loses: 0,
            group,
            locked: false,
        }
    }

    #[test]
    fn the_name_and_the_record_give_the_score() {
        assert_eq!(heuristic("SkyUI", 0, 0), 50);
        assert_eq!(heuristic("Address Library - Core", 0, 0), 28);
        assert_eq!(heuristic("USSEP Compatibility Patch", 0, 0), 76);
        assert_eq!(heuristic("Nemesis_Output", 0, 0), 92);
        assert_eq!(heuristic("Patched Textures", 0, 0), 50, "whole words only");
        assert_eq!(heuristic("X", 1, 9), 44);
        assert_eq!(heuristic("X", 9, 1), 56);
        assert_eq!(heuristic("X", 1, 3), 50, "too few to count");
    }

    #[test]
    fn a_patch_above_its_base_moves_down_inside_its_separator_only() {
        // current: patch, mod, base | output (another separator)
        let rows = [
            row("Some Patch", 0),
            row("Mod", 0),
            row("Base Framework", 0),
            row("Output", 1),
        ];
        let p = plan(&rows, Strictness::Normal, false);
        assert!(p.reorder);
        assert_eq!(
            p.order,
            [2, 1, 0, 3],
            "base first, patch last; the other separator keeps its row"
        );
        assert_eq!(p.rows[0].action, Action::Reorder);
        assert_eq!(p.rows[3].action, Action::Keep);
        assert_eq!(p.rows[0].to, 2);
        // safe with the current order as truth: scores only; inspect: never
        assert!(!plan(&rows, Strictness::Safe, true).reorder);
        assert!(!plan(&rows, Strictness::Inspect, false).reorder);
    }

    #[test]
    fn the_current_order_as_truth_weighs_more_than_the_name() {
        // the patch is off by 76 → the plan sorts, but by the blended score: 34.2 (patch at 0) < 67.6 (base at 100)
        let rows = [row("Some Patch", 0), row("Base", 0)];
        let p = plan(&rows, Strictness::Normal, true);
        assert_eq!((p.rows[0].score, p.rows[1].score), (34.2, 67.6));
        assert_eq!(p.order, [0, 1]);
        // a patch at the bottom and a plain mod at the top swap only when the gap beats the place
        let rows = [
            row("Output", 0),
            row("A", 0),
            row("B", 0),
            row("C", 0),
            row("Base", 0),
        ];
        let p = plan(&rows, Strictness::Aggressive, true);
        assert_eq!(p.rows[0].score, 41.4);
        assert!(
            p.reorder && p.order[0] == 1,
            "Output (41.4) goes below A (22.5 + 13.75)"
        );
    }

    #[test]
    fn locked_rows_keep_their_place() {
        let mut rows = [row("Some Patch", 0), row("Lock", 0), row("Base", 0)];
        rows[1].locked = true;
        let p = plan(&rows, Strictness::Normal, false);
        assert_eq!(p.order, [2, 1, 0]);
        assert_eq!(p.rows[1].action, Action::Locked);
    }

    #[test]
    fn an_order_close_enough_stays() {
        let rows = [row("Base", 0), row("Mod", 0), row("Patch", 0)];
        assert!(!plan(&rows, Strictness::Normal, true).reorder);
    }
}
