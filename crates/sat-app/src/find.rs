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
    profile: Option<sat_core::profile::ContentProfile>,
    content: Option<sat_core::identical::ModContent>,
    stat: Option<ModStat>,
    relations: Option<sat_core::metrics::ModRelations>,
    name: String,
    separator: Option<String>,
    tags: String,
    enabled: bool,
    priority: u32,
    source: Option<sat_core::source::ModSource>,
}

impl ViewParts {
    pub(crate) fn view(&self) -> ModView<'_> {
        ModView {
            metrics: self.metrics.as_ref(),
            profile: self.profile.as_ref(),
            content: self.content.as_ref(),
            stat: self.stat.as_ref(),
            relations: self.relations.as_ref(),
            name: &self.name,
            separator: self.separator.as_deref(),
            tags: Some(&self.tags),
            enabled: self.enabled,
            priority: self.priority,
            source: self.source.as_ref(),
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
    /// The project's chain identities (`@sat:naming.manual`).
    pub fn set_chain_identities(&mut self, ids: Vec<sat_core::naming::ManualIdentity>) {
        self.chain_identities = ids;
        self.chain_identities_rev += 1;
    }

    /// The project's layout of chains (`@sat:layout`).
    pub fn set_chain_layout(&mut self, layout: sat_core::layout::ChainLayout) {
        self.chain_layout = layout;
        self.chain_layout_rev += 1;
    }

    /// The layout as edited (the shell keeps it in the project).
    pub fn chain_layout(&self) -> &sat_core::layout::ChainLayout {
        &self.chain_layout
    }

    /// The project's lineage of chains (`@sat:lineage`).
    pub fn set_chain_lineage(&mut self, lineage: sat_core::lineage::ChainLineage) {
        self.chain_lineage = lineage;
        self.chain_lineage_rev += 1;
    }

    /// The lineage as kept (the shell keeps it in the project).
    pub fn chain_lineage(&self) -> &sat_core::lineage::ChainLineage {
        &self.chain_lineage
    }

    /// The user is working on these exact chains: a followed one takes over its old signature in the layout and
    /// names (`@sat:lineage` — from now on it is known by its own), and every one's members are remembered. True
    /// when anything changed; headings rebuild.
    pub(crate) fn touch(&mut self, chains: &[sat_core::ids::ChainId]) -> bool {
        let ow = self.session.overwrite();
        let heads = self.heads();
        let mut changed = false;
        let mut layout = self.chain_layout.clone();
        for &c in chains {
            let sig = crate::chains::exact_signature(&ow, &self.session, c);
            if let Some(old) = heads.followed_from(sig) {
                layout.rekey(old, sig);
                for m in &mut self.chain_identities {
                    for s in &mut m.sources {
                        if *s == old {
                            *s = sig;
                        }
                    }
                    m.sources.sort_unstable();
                    m.sources.dedup();
                }
                self.chain_lineage.forget(old);
                self.chain_identities_rev += 1;
                changed = true;
            }
            if !self.chain_lineage.knows(sig) {
                changed |= self
                    .chain_lineage
                    .record(sig, crate::chains::exact_members(&ow, &self.session, c));
            }
        }
        if layout != self.chain_layout {
            self.set_chain_layout(layout);
        }
        if changed {
            self.chain_lineage_rev += 1;
        }
        changed
    }

    // @sat:app.orphans
    /// The user's chain records (settled, layout groups, names, lineage) that no chain answers to — now or
    /// followed (`@sat:lineage`): what remains of removed mods.
    pub fn orphans(&mut self) -> Orphans {
        let heads = self.heads();
        let gone = |s: &u64| !heads.answers(*s);
        let decisions = self.session.decisions().signatures().filter(gone).count() as u32;
        let layout = self
            .chain_layout
            .groups()
            .iter()
            .flatten()
            .filter(|s| gone(s))
            .count() as u32;
        let names = self
            .chain_identities
            .iter()
            .filter(|m| m.sources.iter().all(gone))
            .count() as u32;
        Orphans {
            decisions,
            layout,
            names,
        }
    }

    /// Forgets the orphans; how many records went.
    pub(crate) fn prune_orphans(&mut self) -> usize {
        let heads = self.heads();
        let keep = |s: u64| heads.answers(s);
        let mut n = self.session.retain_decisions(keep);
        let mut l = self.chain_layout.clone();
        n += l.retain(keep);
        let before = self.chain_identities.len();
        self.chain_identities
            .retain(|m| m.sources.iter().any(|&s| keep(s)));
        n += before - self.chain_identities.len();
        // lineage of what is gone and no longer referenced
        let refd: rustc_hash::FxHashSet<u64> = self
            .session
            .decisions()
            .signatures()
            .chain(l.groups().iter().flatten().copied())
            .chain(
                self.chain_identities
                    .iter()
                    .flat_map(|m| m.sources.iter().copied()),
            )
            .collect();
        self.chain_lineage.retain(|s| keep(s) || refd.contains(&s));
        if n > 0 {
            self.chain_identities_rev += 1;
            self.set_chain_layout(l);
        }
        n
    }

    /// Signatures the user's layout knows the card's exact chains by (`Head::keys`).
    pub(crate) fn head_sources(&mut self, signature: &str) -> Result<Vec<u64>, crate::AppError> {
        self.heads()
            .iter()
            .find(|h| h.hex() == signature)
            .map(|h| h.keys.clone())
            .ok_or_else(|| crate::AppError::Signature(signature.to_string()))
    }

    /// Exact chains of the card with heading `signature`.
    pub(crate) fn head_chains(
        &mut self,
        signature: &str,
    ) -> Result<Vec<sat_core::ids::ChainId>, crate::AppError> {
        self.heads()
            .iter()
            .find(|h| h.hex() == signature)
            .map(|h| h.chains.clone())
            .ok_or_else(|| crate::AppError::Signature(signature.to_string()))
    }

    /// Puts a new layout in: headings, cards and badges rebuild.
    pub(crate) fn set_layout(&mut self, layout: sat_core::layout::ChainLayout) -> crate::Event {
        self.set_chain_layout(layout);
        self.revision += 1;
        crate::Event::Changed {
            revision: self.revision,
            views: Vec::new(),
            canon: true,
        }
    }

    /// Chain identities as edited (the shell keeps them in the project).
    pub fn chain_identities(&self) -> &[sat_core::naming::ManualIdentity] {
        &self.chain_identities
    }

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
        let (metrics, profile, stat) = match self.session.mod_id(&name) {
            Some(m) => {
                let sum = self.session.table().summary(m);
                tags.extend(sum.found_tags.iter().map(|(t, _)| t.clone()));
                (
                    (!sum.missing).then(|| sum.metrics.clone()),
                    (!sum.missing).then(|| sum.profile.clone()),
                    Some(*ow.stat(m)),
                )
            }
            None => (None, None, None),
        };
        let source = self.sources.get(&name.to_lowercase()).cloned();
        let content = match (self.session.mod_id(&name), self.mod_content()) {
            (Some(m), Some(c)) => c.get(m.idx()).copied(),
            _ => None,
        };
        let relations = self.mod_relations().get(&name.to_lowercase()).copied();
        ViewParts {
            relations,
            source,
            metrics,
            profile,
            content,
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

    /// Where mods come from (`@sat:source`), by name — the project's `sources` handed over by the shell.
    pub fn set_sources(&mut self, sources: Vec<(String, sat_core::source::ModSource)>) {
        self.sources = sources.into_iter().map(|(n, s)| (n.to_lowercase(), s)).collect();
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

/// The user's chain records nothing answers to.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Orphans {
    /// Settled chains.
    pub decisions: u32,
    /// Chains in merge / split groups.
    pub layout: u32,
    /// Names, icons, colours.
    pub names: u32,
}
