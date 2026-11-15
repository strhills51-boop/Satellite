//! The order advisor of a conflict context: rows, warnings, the plan, and applying it. @sat:app.advisor
//!
//! Owns: turning a file context into the advisor's rows (mods and disabled owners in list order, by separator, with
//! their wins and losses in the context — the monolith's `getOverwriteOrderContextRows`), the warnings the monolith
//! gives (scripts, interface, participants in several separators), and `Reorder` — putting mods into an order among
//! their own places inside each separator, one undo step (`@sat:order.book.place`).
//! Not here: the score and the plan (`@sat:advisor`), the context (`@sat:app.context-table`).

use serde::{Deserialize, Serialize};

use sat_core::advisor::{Action, Strictness};
use sat_core::ids::ModId;
use sat_core::order::Key;

use crate::App;

/// One mod of the context with what the advisor says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdviceRow {
    pub key: Option<Key>,
    pub name: String,
    pub separator: Option<String>,
    pub enabled: bool,
    pub wins: u32,
    pub loses: u32,
    pub owned: u32,
    pub heuristic: i32,
    pub score: f64,
    pub current: u32,
    /// New index among the rows.
    pub to: u32,
    pub action: Action,
    /// `скрипты`, `интерфейс`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// The advisor's answer for a context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderAdvice {
    /// Current order.
    pub rows: Vec<AdviceRow>,
    /// Keys in the proposed order (rows without a key — mods out of the list — are left out).
    pub order: Vec<Key>,
    pub reorder: bool,
    /// About the whole context.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl App {
    /// The advisor's plan for the file context of mod `key`; `locked` mods keep their places.
    pub(crate) fn order_advice(
        &mut self,
        key: Key,
        strictness: Strictness,
        current_truth: bool,
        locked: &[Key],
    ) -> Option<OrderAdvice> {
        let t = self.context_table(
            key,
            crate::ColumnOrder::Chain,
            None,
            None,
            (0, usize::MAX),
            (0, 0),
        )?;
        let rows: Vec<crate::TableRow> = t.rows.into_iter().filter(|r| !r.header).collect();
        // warnings from the paths each mod has in the context
        let cx = self.context_of(key)?;
        let ow = self.session.overwrite();
        let table = self.session.table().clone();
        let mut scripts: rustc_hash::FxHashSet<ModId> = Default::default();
        let mut interface: rustc_hash::FxHashSet<ModId> = Default::default();
        for (c, paths) in ow.context_chain_paths(cx) {
            for p in paths {
                let s = table.path_string(p).to_lowercase();
                let sc = s.starts_with("scripts/") || s.ends_with(".pex");
                let ui = s.starts_with("interface/") || s.ends_with(".swf") || s.ends_with(".gfx");
                if !sc && !ui {
                    continue;
                }
                for m in ow.chain_mods(c).iter().copied().chain(ow.idle_owners(&table, p)) {
                    if sc {
                        scripts.insert(m);
                    }
                    if ui {
                        interface.insert(m);
                    }
                }
            }
        }
        let id_of = |name: &str| {
            table
                .mod_names()
                .iter()
                .position(|n| n == name)
                .map(ModId::from_idx)
        };
        // separator groups: contiguous runs of one separator
        let mut groups = Vec::with_capacity(rows.len());
        let mut g = 0u32;
        for (i, r) in rows.iter().enumerate() {
            if i > 0 && rows[i - 1].separator != r.separator {
                g += 1;
            }
            groups.push(g);
        }
        let input: Vec<sat_core::advisor::Row<'_>> = rows
            .iter()
            .zip(&groups)
            .map(|(r, &group)| sat_core::advisor::Row {
                name: &r.name,
                wins: r.wins,
                loses: r.loses,
                group,
                locked: r.key.is_none_or(|k| locked.contains(&k)),
            })
            .collect();
        let plan = sat_core::advisor::plan(&input, strictness, current_truth);
        let mut warnings = Vec::new();
        if g > 0 {
            warnings.push(format!(
                "участники в {} разделителях: моды переставляются только внутри своего",
                g + 1
            ));
        }
        let out: Vec<AdviceRow> = rows
            .into_iter()
            .zip(plan.rows)
            .map(|(r, a)| {
                let m = id_of(&r.name);
                let mut w = Vec::new();
                if m.is_some_and(|m| scripts.contains(&m)) {
                    w.push("скрипты".to_string());
                }
                if m.is_some_and(|m| interface.contains(&m)) {
                    w.push("интерфейс".to_string());
                }
                AdviceRow {
                    key: r.key,
                    name: r.name,
                    separator: r.separator,
                    enabled: r.enabled,
                    wins: r.wins,
                    loses: r.loses,
                    owned: r.owned,
                    heuristic: a.heuristic,
                    score: a.score,
                    current: a.current,
                    to: a.to,
                    action: a.action,
                    warnings: w,
                }
            })
            .collect();
        Some(OrderAdvice {
            order: plan.order.iter().filter_map(|&i| out[i as usize].key).collect(),
            reorder: plan.reorder,
            rows: out,
            warnings,
        })
    }

    // @sat:app.reorder
    /// Puts `keys` (mods) in this relative order among their own places, separator by separator: the places a
    /// separator's mods hold now are refilled in the given order, nothing else moves.
    pub(crate) fn reorder_items(&self, keys: &[Key]) -> Vec<(Key, usize)> {
        let book = self.session.book();
        let mut by: Vec<(Option<Key>, Vec<Key>)> = Vec::new();
        for &k in keys {
            let s = book.separator_of(k);
            match by.iter_mut().find(|x| x.0 == s) {
                Some(x) => x.1.push(k),
                None => by.push((s, vec![k])),
            }
        }
        let mut items = Vec::with_capacity(keys.len());
        for (_, ks) in by {
            let mut places: Vec<usize> = ks.iter().map(|&k| book.pos(k)).collect();
            places.sort_unstable();
            items.extend(ks.into_iter().zip(places));
        }
        items
    }
}

#[cfg(test)]
mod tests {
    use sat_core::advisor::{Action, Strictness};

    use crate::{Command, Query, Reply};

    #[test]
    fn the_advisor_puts_a_patch_after_what_it_patches_and_reorder_applies_it() {
        // Patch (top of the list, loses) and Base both have x; Patch is above Base in the list
        let mut a = crate::tests::app_of(["Some Patch", "Base"]);
        let key = a.session().book().key_of("Base").unwrap();
        let Reply::OrderAdvice { advice: Some(adv) } = a
            .query(Query::OrderAdvice {
                key,
                strictness: Strictness::Normal,
                current_truth: false,
                locked: vec![],
            })
            .unwrap()
        else {
            panic!()
        };
        let names: Vec<&str> = adv.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Some Patch", "Base"]);
        assert!(adv.reorder);
        assert_eq!(adv.rows[0].action, Action::Reorder);
        let base = a.session().book().key_of("Base").unwrap();
        let patch = a.session().book().key_of("Some Patch").unwrap();
        assert_eq!(adv.order, [base, patch]);
        a.command(Command::Reorder {
            keys: adv.order.clone(),
        })
        .unwrap();
        let b = a.session().book();
        assert!(b.pos(base) < b.pos(patch), "applied in one step");
        a.command(Command::Undo).unwrap();
        let b = a.session().book();
        assert!(b.pos(patch) < b.pos(base), "and undone in one");
    }
}
