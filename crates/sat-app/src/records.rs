//! The record layer in the window: what a shell computed, as the window asks for it. @sat:app.records
//!
//! Owns: the record report the shell hands over (`@sat:records`: chains, per-plugin stats, plugins not read), its
//! state (not asked for, being computed, ready), a page of record chains (optionally of one plugin), and the record
//! stats a plugin row shows.
//! Not here: reading plugins and building the index (`@sat:scan.records`, `@sat:records.index` — the shell does it,
//! off the request loop).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use sat_core::records::{PluginRecordStat, RecordChain, RecordContext, RecordIndex};

use crate::context::{ContextTable, TableChain, TableCol, TableRow};

use crate::App;

/// What the record layer found, ready to show.
#[derive(Debug, Clone, Default)]
pub struct RecordReport {
    pub records: usize,
    /// Records some plugin overrides, and those two or more override (conflicts).
    pub overridden: usize,
    pub conflicts: usize,
    pub plugins: usize,
    pub chains: Vec<RecordChain>,
    /// By plugin name in lower case.
    pub stats: HashMap<String, PluginRecordStat>,
    /// Plugins that could not be read, with why.
    pub unread: Vec<String>,
    /// The index and the conflict contexts (`@sat:records.contexts`), for the conflict table.
    pub index: std::sync::Arc<RecordIndex>,
    pub contexts: Vec<RecordContext>,
}

impl RecordReport {
    pub fn from_index(ix: RecordIndex, unread: Vec<String>) -> Self {
        let ix = std::sync::Arc::new(ix);
        let stats = sat_core::records::plugin_stats(&ix);
        let chains = sat_core::records::chains(&ix);
        let contexts = sat_core::records::contexts(&ix, &chains);
        Self {
            records: ix.records(),
            overridden: ix.overridden(),
            conflicts: ix.conflicts(),
            plugins: ix.plugins.len(),
            chains,
            stats: ix
                .plugins
                .iter()
                .zip(stats)
                .map(|(p, s)| (p.to_lowercase(), s))
                .collect(),
            unread,
            contexts,
            index: ix,
        }
    }
}

/// A record context for the conflict window's selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordContextItem {
    pub name: String,
    pub hue: u16,
    pub plugins: usize,
    pub chains: usize,
    pub records: u32,
    pub blind: u32,
    /// Its plugins (lower case), so a plugin finds its context.
    pub members: Vec<String>,
}

/// Where the record layer is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// The shell does not compute it.
    #[default]
    Off,
    /// Plugins are being read.
    Pending,
    Ready,
}

// @sat:app.records.page
/// A page of record chains.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordPage {
    pub state: RecordState,
    pub records: usize,
    pub overridden: usize,
    pub conflicts: usize,
    pub plugins: usize,
    /// Chains matching the filter, and the page of them.
    pub total: usize,
    pub chains: Vec<RecordChain>,
    pub unread: Vec<String>,
}

impl App {
    /// The shell starts computing the record layer.
    pub fn records_pending(&mut self) {
        self.record_state = RecordState::Pending;
    }

    /// The shell's record report.
    pub fn set_records(&mut self, report: RecordReport) {
        self.records = std::sync::Arc::new(report);
        self.record_state = RecordState::Ready;
        self.domain_inputs.records_rev += 1;
    }

    /// Record chains, conflicts first, largest first; with `plugin` only those it is in, with `conflicts_only` no
    /// single overrides.
    pub(crate) fn record_page(
        &self,
        plugin: Option<&str>,
        conflicts_only: bool,
        offset: usize,
        limit: usize,
    ) -> RecordPage {
        let r = &self.records;
        let want = plugin.map(str::to_lowercase);
        let hit = |c: &&RecordChain| {
            (c.conflict || !conflicts_only)
                && want
                    .as_ref()
                    .is_none_or(|w| c.stack.iter().any(|p| p.to_lowercase() == *w))
        };
        RecordPage {
            state: self.record_state,
            records: r.records,
            overridden: r.overridden,
            conflicts: r.conflicts,
            plugins: r.plugins,
            total: r.chains.iter().filter(hit).count(),
            chains: r
                .chains
                .iter()
                .filter(hit)
                .skip(offset)
                .take(limit)
                .cloned()
                .collect(),
            unread: r.unread.clone(),
        }
    }

    fn record_context_name(&self, cx: &RecordContext) -> String {
        let ix = &self.records.index;
        let name = |p: u32| ix.plugins[p as usize].clone();
        match cx.plugins.as_slice() {
            [] => String::new(),
            [a] => name(*a),
            [a, .., b] => format!("{} -> {} ({} plugins)", name(*a), name(*b), cx.plugins.len()),
        }
    }

    // @sat:app.records.contexts
    /// Record conflict contexts, largest first.
    pub(crate) fn record_contexts(&self) -> Vec<RecordContextItem> {
        self.records
            .contexts
            .iter()
            .map(|cx| {
                let name = self.record_context_name(cx);
                RecordContextItem {
                    hue: sat_core::identity::identity(
                        cx.plugins
                            .iter()
                            .map(|&p| self.records.index.plugins[p as usize].as_str()),
                    )
                    .hue,
                    name,
                    plugins: cx.plugins.len(),
                    chains: cx.chains.len(),
                    records: cx.records,
                    blind: cx.blind_records,
                    members: cx
                        .plugins
                        .iter()
                        .map(|&p| self.records.index.plugins[p as usize].to_lowercase())
                        .collect(),
                }
            })
            .collect()
    }

    // @sat:app.records.table
    /// A window of a record context's conflict table, in the file table's shape (`ContextTable`): rows — its plugins in
    /// load order, columns — its records grouped by chain (blind first), cells — `w` the record the game takes, `l` an
    /// override taken over, `s` an override equal to the winner's, `o` the record's own plugin, `-` not there.
    pub(crate) fn record_table(
        &self,
        context: usize,
        rows: (usize, usize),
        cols: (usize, usize),
    ) -> Option<ContextTable> {
        let r = &self.records;
        let cx = r.contexts.get(context)?;
        let ix = &r.index;
        let cols_total: usize = cx.chains.iter().map(|&c| r.chains[c].keys.len()).sum();
        // the window's columns: (chain position, key)
        let mut window: Vec<(u32, u64)> = Vec::new();
        let mut skip = cols.0;
        for (pos, &c) in cx.chains.iter().enumerate() {
            let keys = &r.chains[c].keys;
            if skip >= keys.len() {
                skip -= keys.len();
                continue;
            }
            for &k in keys.iter().skip(skip) {
                if window.len() >= cols.1 {
                    break;
                }
                window.push((pos as u32, k));
            }
            skip = 0;
            if window.len() >= cols.1 {
                break;
            }
        }
        let providers: Vec<Vec<sat_core::records::Provider>> =
            window.iter().map(|&(_, k)| ix.providers(k)).collect();
        let mut chain_starts = Vec::new();
        let mut at = 0u32;
        for &c in &cx.chains {
            chain_starts.push(at);
            at += r.chains[c].keys.len() as u32;
        }
        let mut shown: Vec<u32> = window.iter().map(|w| w.0).collect();
        shown.dedup();
        let chains = shown
            .iter()
            .map(|&pos| {
                let c = &r.chains[cx.chains[pos as usize]];
                let id = sat_core::identity::identity(c.stack.iter().map(String::as_str));
                TableChain {
                    index: pos,
                    signature: {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::hash::DefaultHasher::new();
                        c.stack.hash(&mut h);
                        format!("{:016x}", h.finish())
                    },
                    done: false,
                    emoji: if c.blind {
                        "⚠️".into()
                    } else {
                        id.emoji.to_string()
                    },
                    hue: id.hue,
                    name: format!(
                        "{} {}",
                        if c.blind { "вслепую" } else { "поверх" },
                        c.stack.join(" -> ")
                    ),
                    files: c.records,
                    depth: c.stack.len(),
                }
            })
            .collect();
        let table_rows = cx
            .plugins
            .iter()
            .skip(rows.0)
            .take(rows.1)
            .map(|&p| {
                let st = r
                    .stats
                    .get(&ix.plugins[p as usize].to_lowercase())
                    .copied()
                    .unwrap_or_default();
                let cells: String = providers
                    .iter()
                    .map(|pr| {
                        let last = pr.last().map(|x| (x.plugin, x.hash));
                        match pr.iter().find(|x| x.plugin == p) {
                            None => '-',
                            Some(x) if x.origin => 'o',
                            Some(x) if Some(x.plugin) == last.map(|l| l.0) => 'w',
                            Some(x) if x.hash.is_some() && x.hash == last.and_then(|l| l.1) => 's',
                            Some(_) => 'l',
                        }
                    })
                    .collect();
                TableRow {
                    header: false,
                    key: None,
                    name: ix.plugins[p as usize].clone(),
                    pos: Some(p),
                    separator: None,
                    enabled: true,
                    wins: st.wins,
                    loses: st.loses,
                    cells,
                }
            })
            .collect();
        let item = self.record_contexts().into_iter().nth(context)?;
        Some(ContextTable {
            name: item.name,
            hue: item.hue,
            rows_total: cx.plugins.len(),
            cols_total,
            chains,
            chains_total: cx.chains.len(),
            chain_starts,
            rows: table_rows,
            cols: window
                .iter()
                .zip(&providers)
                .map(|(&(pos, k), pr)| TableCol {
                    path: format!(
                        "{} {}",
                        ix.label(k),
                        pr.last()
                            .map(|x| String::from_utf8_lossy(&x.signature).into_owned())
                            .unwrap_or_default()
                    ),
                    chain: pos,
                })
                .collect(),
        })
    }

    /// What a plugin does in the record layer (`None` before it is computed or for a plugin not loaded).
    pub(crate) fn plugin_records(&self, name: &str) -> Option<PluginRecordStat> {
        self.records.stats.get(&name.to_lowercase()).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Query, Reply};
    use sat_core::records::{PluginRecords, RecordFact};

    #[test]
    fn the_window_gets_chains_by_plugin_and_stats_per_plugin() {
        let mut a = crate::tests::app_of(["A", "B"]);
        assert_eq!(a.record_page(None, false, 0, 10).state, RecordState::Off);
        a.records_pending();
        assert_eq!(a.record_page(None, false, 0, 10).state, RecordState::Pending);
        let rec = |id: u32, h: u64| RecordFact {
            flags: 0,
            form_id: id,
            signature: *b"NPC_",
            hash: Some(h),
        };
        let x = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(1, 1), rec(2, 2)],
        };
        let y = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(1, 3)],
        };
        let ix = RecordIndex::build(&[("X.esp".into(), &x), ("Y.esp".into(), &y)]);
        a.set_records(RecordReport::from_index(ix, vec!["Z.esp: not found".into()]));
        let p = a.record_page(None, false, 0, 10);
        assert_eq!(
            (p.state, p.records, p.conflicts, p.total),
            (RecordState::Ready, 3, 1, 1)
        );
        assert_eq!(p.chains[0].stack, ["X.esp", "Y.esp"]);
        assert_eq!(a.record_page(Some("y.esp"), false, 0, 10).total, 1);
        assert_eq!(a.record_page(Some("Other.esp"), false, 0, 10).total, 0);
        assert_eq!(a.plugin_records("X.ESP").map(|s| s.loses), Some(1));
        // the conflict table of the records: X and Y over Skyrim's record, Y takes it
        let Reply::RecordContexts { contexts } = a.query(Query::RecordContexts).unwrap() else {
            panic!()
        };
        assert_eq!(
            (contexts.len(), contexts[0].plugins, contexts[0].records),
            (1, 2, 1)
        );
        let Reply::ContextTable { table: Some(t) } = a
            .query(Query::RecordTable {
                context: 0,
                row_start: 0,
                row_count: 10,
                col_start: 0,
                col_count: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!((t.rows_total, t.cols_total, t.chains.len()), (2, 1, 1));
        assert_eq!(
            t.rows.iter().map(|r| r.cells.as_str()).collect::<Vec<_>>(),
            ["l", "w"]
        );
        assert_eq!(t.cols[0].path, "skyrim.esm:000001 NPC_");
        assert!(t.chains[0].name.starts_with("вслепую"));
        // the same chain as a card, and its teeth
        let Reply::ChainCards { cards: Some(cards) } = a.query(Query::RecordCards { context: 0 }).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            (
                cards.len(),
                cards[0].files,
                cards[0].blind,
                cards[0].stack.clone()
            ),
            (1, 1, true, vec!["X.esp".to_string(), "Y.esp".to_string()])
        );
        let Reply::ChainCard { card: Some(d) } = a
            .query(Query::RecordCard {
                context: 0,
                index: 0,
                limit: 10,
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            d.members
                .iter()
                .map(|m| (m.name.as_str(), m.teeth.as_str()))
                .collect::<Vec<_>>(),
            [("X.esp", "l"), ("Y.esp", "w")]
        );
        assert_eq!(
            (d.items.clone(), d.items_total),
            (vec!["skyrim.esm:000001 NPC_".to_string()], 1)
        );
        assert!(a.record_cards(5).is_none());
    }
}
