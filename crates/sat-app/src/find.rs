//! Mods by condition, and what rules say about a mod: the metrics view of the window. @sat:app.find
//!
//! Owns: one mod's metrics view (`ModView`: scan aggregates, overwrite counters, tags — the user's and the found
//! ones), finding the mods an expression holds for (`Query::Find`), and the stickers the project's rules give a mod.
//! Not here: the language (`@sat:expr`), the metrics (`@sat:metrics`), sticker rules (`@sat:sticker-rules`).

use serde::{Deserialize, Serialize};

use sat_core::expr::Expr;
use sat_core::facts::ModMetrics;
use sat_core::metrics::ModView;
use sat_core::order::Key;
use sat_core::overwrite::ModStat;

use crate::{App, AppError};

/// What a view of one mod is made of (owned: the view borrows it).
pub(crate) struct ViewParts {
    metrics: Option<ModMetrics>,
    stat: Option<ModStat>,
    name: String,
    separator: Option<String>,
    tags: String,
    enabled: bool,
    priority: u32,
}

impl ViewParts {
    pub(crate) fn view(&self) -> ModView<'_> {
        ModView {
            metrics: self.metrics.as_ref(),
            stat: self.stat.as_ref(),
            name: &self.name,
            separator: self.separator.as_deref(),
            tags: Some(&self.tags),
            enabled: self.enabled,
            priority: self.priority,
        }
    }
}

/// Mods an expression holds for, in list order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Found {
    pub keys: Vec<Key>,
    /// All matches (the list is cut at the limit).
    pub total: usize,
}

impl App {
    /// The project's sticker rules (`@sat:sticker-rules`); returns the rules that did not compile.
    pub fn set_sticker_rules(&mut self, rules: &[sat_core::sticker_rules::StickerRule]) -> Vec<String> {
        let (r, errors) = sat_core::sticker_rules::StickerRules::compile(rules);
        self.sticker_rules = r;
        errors
    }

    // @sat:app.find.view
    pub(crate) fn view_parts(&mut self, key: Key) -> ViewParts {
        let ow = self.session.overwrite();
        let book = self.session.book();
        let name = book.name(key).to_string();
        let separator = book
            .separator_of(key)
            .map(|s| book.name(s).trim_end_matches("_separator").to_string());
        let enabled = book.enabled(key);
        let priority = book.pos(key) as u32;
        let mut tags: Vec<String> = book.marks(key).map(|m| m.tags.clone()).unwrap_or_default();
        let (metrics, stat) = match self.session.mod_id(&name) {
            Some(m) => {
                let sum = self.session.table().summary(m);
                tags.extend(sum.found_tags.iter().map(|(t, _)| t.clone()));
                ((!sum.missing).then(|| sum.metrics.clone()), Some(*ow.stat(m)))
            }
            None => (None, None),
        };
        ViewParts {
            metrics,
            stat,
            name,
            separator,
            tags: tags.join(", "),
            enabled,
            priority,
        }
    }

    /// The project's counters (`@sat:counters`); returns the definitions that did not compile.
    pub fn set_counters(&mut self, defs: &[sat_core::counters::CounterDef]) -> Vec<String> {
        let (c, errors) = sat_core::counters::Counters::compile(defs);
        self.counters = c;
        errors
    }

    /// System stickers by mod name (`Project::system_stickers`: `planned`, `new`), handed over by the shell.
    pub fn set_system_stickers(&mut self, stickers: Vec<(String, &'static str)>) {
        self.system_stickers.clear();
        for (n, s) in stickers {
            self.system_stickers
                .entry(n.to_lowercase())
                .or_default()
                .push(s.to_string());
        }
    }

    /// Stickers the project's rules give the mod at `key`, then the project's own (`planned`, `new`).
    pub(crate) fn rule_stickers(&mut self, key: Key) -> Vec<String> {
        let name = self.session.book().name(key).to_lowercase();
        let mut out = if self.sticker_rules.is_empty() {
            Vec::new()
        } else {
            let parts = self.view_parts(key);
            self.sticker_rules.stickers(&parts.view())
        };
        out.extend(self.system_stickers.get(&name).into_iter().flatten().cloned());
        out
    }

    // @sat:app.find.query
    /// Mods for which `source` holds, in list order, at most `limit` keys.
    pub(crate) fn find(&mut self, source: &str, limit: usize) -> Result<Found, AppError> {
        let expr = Expr::compile_with(source, &sat_core::metrics::resolve)
            .map_err(|e| AppError::Expr(e.to_string()))?;
        let keys: Vec<Key> = {
            let book = self.session.book();
            book.entries()
                .filter(|(_, kind, _)| *kind == sat_core::canon::EntryKind::Mod)
                .filter_map(|(n, _, _)| book.key_of(n))
                .collect()
        };
        let mut found = Found {
            keys: Vec::new(),
            total: 0,
        };
        for k in keys {
            let parts = self.view_parts(k);
            if expr.test(&|id| parts.view().get(id)) {
                found.total += 1;
                if found.keys.len() < limit {
                    found.keys.push(k);
                }
            }
        }
        Ok(found)
    }
}
