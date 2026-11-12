//! The record layer in the window: what a shell computed, as the window asks for it. @sat:app.records
//!
//! Owns: the record report the shell hands over (`@sat:records`: chains, per-plugin stats, plugins not read), its
//! state (not asked for, being computed, ready), a page of record chains (optionally of one plugin), and the record
//! stats a plugin row shows.
//! Not here: reading plugins and building the index (`@sat:scan.records`, `@sat:records.index` — the shell does it,
//! off the request loop).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use sat_core::records::{PluginRecordStat, RecordChain, RecordIndex};

use crate::App;

/// What the record layer found, ready to show.
#[derive(Debug, Clone, Default)]
pub struct RecordReport {
    pub records: usize,
    pub conflicts: usize,
    pub plugins: usize,
    pub chains: Vec<RecordChain>,
    /// By plugin name in lower case.
    pub stats: HashMap<String, PluginRecordStat>,
    /// Plugins that could not be read, with why.
    pub unread: Vec<String>,
}

impl RecordReport {
    pub fn from_index(ix: &RecordIndex, unread: Vec<String>) -> Self {
        let stats = sat_core::records::plugin_stats(ix);
        Self {
            records: ix.records(),
            conflicts: ix.conflicts(),
            plugins: ix.plugins.len(),
            chains: sat_core::records::chains(ix),
            stats: ix
                .plugins
                .iter()
                .zip(stats)
                .map(|(p, s)| (p.to_lowercase(), s))
                .collect(),
            unread,
        }
    }
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
    }

    /// Record chains, largest first; with `plugin` only those it is in.
    pub(crate) fn record_page(&self, plugin: Option<&str>, offset: usize, limit: usize) -> RecordPage {
        let r = &self.records;
        let want = plugin.map(str::to_lowercase);
        let hit = |c: &&RecordChain| {
            want.as_ref()
                .is_none_or(|w| c.stack.iter().any(|p| p.to_lowercase() == *w))
        };
        RecordPage {
            state: self.record_state,
            records: r.records,
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

    /// What a plugin does in the record layer (`None` before it is computed or for a plugin not loaded).
    pub(crate) fn plugin_records(&self, name: &str) -> Option<PluginRecordStat> {
        self.records.stats.get(&name.to_lowercase()).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sat_core::records::{PluginRecords, RecordFact};

    #[test]
    fn the_window_gets_chains_by_plugin_and_stats_per_plugin() {
        let mut a = crate::tests::app_of(["A", "B"]);
        assert_eq!(a.record_page(None, 0, 10).state, RecordState::Off);
        a.records_pending();
        assert_eq!(a.record_page(None, 0, 10).state, RecordState::Pending);
        let rec = |id: u32, h: u64| RecordFact {
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
        a.set_records(RecordReport::from_index(&ix, vec!["Z.esp: not found".into()]));
        let p = a.record_page(None, 0, 10);
        assert_eq!(
            (p.state, p.records, p.conflicts, p.total),
            (RecordState::Ready, 3, 1, 1)
        );
        assert_eq!(p.chains[0].stack, ["X.esp", "Y.esp"]);
        assert_eq!(a.record_page(Some("y.esp"), 0, 10).total, 1);
        assert_eq!(a.record_page(Some("Other.esp"), 0, 10).total, 0);
        assert_eq!(a.plugin_records("X.ESP").map(|s| s.loses), Some(1));
    }
}
