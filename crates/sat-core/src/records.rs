//! The record layer: plugins' records as facts, their overrides as conflicts. @sat:records
//!
//! The same model as file paths, keyed by record instead of path.
//!
//! Owns: a plugin's records as facts (FormID, signature, content hash), the record index over the loaded plugins
//! (key: the plugin a record comes from + its object id; providers: every plugin that has it, in load order; winner:
//! the last), record conflicts grouped into chains by their plugin stack, and what each plugin wins, loses and copies
//! unchanged.
//! Not here: reading plugins (`@sat:formats.plugin.records`, `@sat:scan.records`), the plugin list and load order
//! (`@sat:plugins.list`), file paths (`@sat:table`).
//!
//! A FormID in a file names its origin by the index of the plugin's masters (the plugin itself after them), so the
//! same record has the same key in every plugin that touches it. The game takes a record whole from the last plugin
//! that has it: what earlier plugins changed in it is lost unless the last one carries it too — the conflicts xEdit
//! shows. A record equal to the one it overrides (an "ITM", identical to master) is still an override: it is
//! counted and shown, and named for what it is. Base masters are read from the game's `Data` when they are there;
//! without them, overrides of vanilla records by a single plugin are not conflicts and their content is not known.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

/// One record of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordFact {
    pub form_id: u32,
    pub signature: [u8; 4],
    /// Hash of the record's (unpacked) data; `None` — it did not unpack.
    pub hash: Option<u64>,
}

/// A plugin's records with its masters (they resolve the FormIDs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRecords {
    pub masters: Vec<String>,
    pub records: Vec<RecordFact>,
}

fn sig(s: [u8; 4]) -> String {
    String::from_utf8_lossy(&s).into_owned()
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    key: u64,
    /// Load-order index into `RecordIndex::plugins`.
    plugin: u32,
    signature: [u8; 4],
    hash: Option<u64>,
}

// @sat:records.index
/// The records of the loaded plugins, by record.
#[derive(Debug, Default)]
pub struct RecordIndex {
    /// Loaded plugins, in load order.
    pub plugins: Vec<String>,
    /// Plugins records come from (lower case), by origin id.
    origins: Vec<String>,
    /// Sorted by (key, plugin).
    entries: Vec<Entry>,
    /// Records with more than one provider: (first entry, count).
    contested: Vec<(u32, u32)>,
}

impl RecordIndex {
    /// Builds the index from plugins in load order: (name, records).
    pub fn build(plugins: &[(String, &PluginRecords)]) -> Self {
        let mut origin_id: HashMap<String, u32> = HashMap::new();
        let mut origins: Vec<String> = Vec::new();
        let mut id = |name: &str| -> u32 {
            let k = name.to_lowercase();
            *origin_id.entry(k.clone()).or_insert_with(|| {
                origins.push(k);
                (origins.len() - 1) as u32
            })
        };
        let mut entries = Vec::with_capacity(plugins.iter().map(|p| p.1.records.len()).sum());
        for (i, (name, recs)) in plugins.iter().enumerate() {
            let own = id(name);
            let masters: Vec<u32> = recs.masters.iter().map(|m| id(m)).collect();
            for r in &recs.records {
                let at = (r.form_id >> 24) as usize;
                let origin = masters.get(at).copied().unwrap_or(own);
                entries.push(Entry {
                    key: (u64::from(origin) << 32) | u64::from(r.form_id & 0x00FF_FFFF),
                    plugin: i as u32,
                    signature: r.signature,
                    hash: r.hash,
                });
            }
        }
        entries.sort_unstable_by_key(|e| (e.key, e.plugin));
        // a plugin listing a record twice keeps its last copy
        entries.dedup_by(|b, a| a.key == b.key && a.plugin == b.plugin);
        let mut contested = Vec::new();
        let mut i = 0;
        while i < entries.len() {
            let j = entries[i..]
                .iter()
                .position(|e| e.key != entries[i].key)
                .map_or(entries.len(), |n| i + n);
            if j - i > 1 {
                contested.push((i as u32, (j - i) as u32));
            }
            i = j;
        }
        Self {
            plugins: plugins.iter().map(|p| p.0.clone()).collect(),
            origins,
            entries,
            contested,
        }
    }

    pub fn records(&self) -> usize {
        self.entries.len()
    }

    /// Records more than one plugin has.
    pub fn conflicts(&self) -> usize {
        self.contested.len()
    }

    fn group(&self, g: (u32, u32)) -> &[Entry] {
        &self.entries[g.0 as usize..(g.0 + g.1) as usize]
    }

    /// The record a conflict is about: its origin plugin and FormID within it, e.g. `Skyrim.esm:012E49`.
    fn label(&self, key: u64) -> String {
        format!("{}:{:06X}", self.origins[(key >> 32) as usize], key & 0x00FF_FFFF)
    }
}

// @sat:records.plugin-stat
/// What a plugin does in the record layer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRecordStat {
    pub records: u32,
    /// Records that come from this plugin (not overrides).
    pub new_records: u32,
    /// Records of other plugins it overrides.
    pub overrides: u32,
    /// Contested records it is the last to have (the game takes them from it).
    pub wins: u32,
    /// Contested records a later plugin takes over.
    pub loses: u32,
    /// Overrides equal to the record they override (ITM): still overrides, carrying no change.
    pub unchanged: u32,
}

/// Per plugin (load order), over the whole index.
pub fn plugin_stats(ix: &RecordIndex) -> Vec<PluginRecordStat> {
    let mut out = vec![PluginRecordStat::default(); ix.plugins.len()];
    let own_origin: Vec<Option<u32>> = ix
        .plugins
        .iter()
        .map(|p| {
            ix.origins
                .iter()
                .position(|o| *o == p.to_lowercase())
                .map(|x| x as u32)
        })
        .collect();
    for e in &ix.entries {
        let s = &mut out[e.plugin as usize];
        s.records += 1;
        if own_origin[e.plugin as usize] == Some((e.key >> 32) as u32) {
            s.new_records += 1;
        } else {
            s.overrides += 1;
        }
    }
    for &g in &ix.contested {
        let es = ix.group(g);
        let last = es.len() - 1;
        for (k, e) in es.iter().enumerate() {
            let s = &mut out[e.plugin as usize];
            if k == last {
                s.wins += 1;
            } else {
                s.loses += 1;
            }
            if k > 0 && e.hash.is_some() && e.hash == es[k - 1].hash {
                s.unchanged += 1;
            }
        }
    }
    out
}

// @sat:records.chains
/// Record conflicts with the same plugin stack: one chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordChain {
    /// Plugins in load order, the winner last.
    pub stack: Vec<String>,
    pub records: u32,
    /// Records by signature (`NPC_`, `LVLI`, …).
    pub by_signature: BTreeMap<String, u32>,
    /// Records where every copy is the winner's (nothing is lost, still a conflict).
    pub equal_to_winner: u32,
    /// Records with a copy whose content is not known.
    pub unknown: u32,
    /// A few of its records (`Skyrim.esm:012E49`).
    pub samples: Vec<String>,
}

/// Chains, largest first.
pub fn chains(ix: &RecordIndex) -> Vec<RecordChain> {
    let mut by_stack: HashMap<Vec<u32>, RecordChain> = HashMap::new();
    for &g in &ix.contested {
        let es = ix.group(g);
        let stack: Vec<u32> = es.iter().map(|e| e.plugin).collect();
        let c = by_stack.entry(stack.clone()).or_insert_with(|| RecordChain {
            stack: stack.iter().map(|&p| ix.plugins[p as usize].clone()).collect(),
            records: 0,
            by_signature: BTreeMap::new(),
            equal_to_winner: 0,
            unknown: 0,
            samples: Vec::new(),
        });
        let winner = es[es.len() - 1];
        c.records += 1;
        *c.by_signature.entry(sig(winner.signature)).or_default() += 1;
        if es.iter().any(|e| e.hash.is_none()) {
            c.unknown += 1;
        } else if es.iter().all(|e| e.hash == winner.hash) {
            c.equal_to_winner += 1;
        }
        if c.samples.len() < 5 {
            c.samples.push(ix.label(winner.key));
        }
    }
    let mut v: Vec<RecordChain> = by_stack.into_values().collect();
    v.sort_by(|a, b| b.records.cmp(&a.records).then_with(|| a.stack.cmp(&b.stack)));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(form_id: u32, s: &[u8; 4], hash: u64) -> RecordFact {
        RecordFact {
            form_id,
            signature: *s,
            hash: Some(hash),
        }
    }

    #[test]
    fn overrides_meet_on_the_same_record_whatever_index_they_use() {
        let base = PluginRecords {
            masters: vec![],
            records: vec![rec(0x0001_2E49, b"NPC_", 1), rec(0x0000_0800, b"WEAP", 2)],
        };
        // A: Skyrim is master 0; edits the NPC, copies the weapon unchanged, adds its own record
        let a = PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![
                rec(0x0001_2E49, b"NPC_", 10),
                rec(0x0000_0800, b"WEAP", 2),
                rec(0x0100_0801, b"LVLI", 3),
            ],
        };
        // B: masters Skyrim, A (A is index 1): edits the NPC again and A's list
        let b = PluginRecords {
            masters: vec!["Skyrim.esm".into(), "A.esp".into()],
            records: vec![rec(0x0001_2E49, b"NPC_", 20), rec(0x0100_0801, b"LVLI", 3)],
        };
        let ix = RecordIndex::build(&[
            ("Skyrim.esm".into(), &base),
            ("A.esp".into(), &a),
            ("B.esp".into(), &b),
        ]);
        assert_eq!(ix.records(), 7);
        assert_eq!(ix.conflicts(), 3, "NPC (3 plugins), WEAP (2), A's LVLI (2)");
        let st = plugin_stats(&ix);
        assert_eq!((st[1].new_records, st[1].overrides), (1, 2));
        assert_eq!(
            (st[1].wins, st[1].loses, st[1].unchanged),
            (1, 2, 1),
            "A wins WEAP with an unchanged copy"
        );
        assert_eq!(
            (st[2].wins, st[2].loses, st[2].unchanged),
            (2, 0, 1),
            "B's LVLI equals A's"
        );
        let ch = chains(&ix);
        assert_eq!(ch.len(), 3);
        let npc = ch.iter().find(|c| c.stack.len() == 3).unwrap();
        assert_eq!(npc.by_signature["NPC_"], 1);
        assert_eq!(npc.samples, ["skyrim.esm:012E49"]);
        let lvli = ch.iter().find(|c| c.stack == ["A.esp", "B.esp"]).unwrap();
        assert_eq!(lvli.equal_to_winner, 1, "equal, still a conflict");
    }
}
