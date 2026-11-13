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
//! counted and shown, and named for what it is. One plugin over a record's own plugin is an override (what a mod
//! is for); two or more over it are a conflict — one's changes are lost. Both are shown, conflicts first. Base masters are read from the game's `Data` when they are there;
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
    /// Each loaded plugin's own origin id (its new records carry it).
    plugin_origin: Vec<u32>,
    /// Each loaded plugin's masters, as origin ids.
    plugin_masters: Vec<Vec<u32>>,
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
        let mut plugin_origin = Vec::with_capacity(plugins.len());
        let mut plugin_masters = Vec::with_capacity(plugins.len());
        for (i, (name, recs)) in plugins.iter().enumerate() {
            let own = id(name);
            plugin_origin.push(own);
            let masters: Vec<u32> = recs.masters.iter().map(|m| id(m)).collect();
            plugin_masters.push(masters.clone());
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
            plugin_origin,
            plugin_masters,
        }
    }

    pub fn records(&self) -> usize {
        self.entries.len()
    }

    /// Records more than one plugin has (an override at least).
    pub fn overridden(&self) -> usize {
        self.contested.len()
    }

    /// Records two or more plugins override (beyond the plugin they come from): one's changes are lost.
    pub fn conflicts(&self) -> usize {
        self.contested
            .iter()
            .filter(|&&g| self.overriders(self.group(g)) >= 2)
            .count()
    }

    /// Whether an entry is the record's own plugin (not an override).
    fn is_origin(&self, e: &Entry) -> bool {
        self.plugin_origin[e.plugin as usize] == (e.key >> 32) as u32
    }

    fn overriders(&self, es: &[Entry]) -> usize {
        es.iter().filter(|e| !self.is_origin(e)).count()
    }

    // @sat:records.blind
    /// Whether some overriding plugin takes a record over from an earlier overriding plugin it does not have as a
    /// master: it was not made with it in view, and the earlier one's changes are lost unknowingly. A plugin built on
    /// the previous one (a patch, an addon) overrides it on purpose.
    fn blind(&self, es: &[Entry]) -> bool {
        let over: Vec<u32> = es
            .iter()
            .filter(|e| !self.is_origin(e))
            .map(|e| e.plugin)
            .collect();
        over.windows(2).any(|w| {
            let prev = self.plugin_origin[w[0] as usize];
            !self.plugin_masters[w[1] as usize].contains(&prev)
        })
    }

    fn group(&self, g: (u32, u32)) -> &[Entry] {
        &self.entries[g.0 as usize..(g.0 + g.1) as usize]
    }

    // @sat:records.providers
    /// The plugins that have a record, in load order.
    pub fn providers(&self, key: u64) -> Vec<Provider> {
        let at = self.entries.partition_point(|e| e.key < key);
        self.entries[at..]
            .iter()
            .take_while(|e| e.key == key)
            .map(|e| Provider {
                plugin: e.plugin,
                origin: self.is_origin(e),
                hash: e.hash,
                signature: e.signature,
            })
            .collect()
    }

    /// The record a conflict is about: its origin plugin and FormID within it, e.g. `Skyrim.esm:012E49`.
    pub fn label(&self, key: u64) -> String {
        format!("{}:{:06X}", self.origins[(key >> 32) as usize], key & 0x00FF_FFFF)
    }
}

/// A plugin that has a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// Load-order index into `RecordIndex::plugins`.
    pub plugin: u32,
    /// The record comes from this plugin (not an override).
    pub origin: bool,
    pub hash: Option<u64>,
    pub signature: [u8; 4],
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
    /// Its own records that a later plugin overrides (what the override is for, not a loss between mods).
    pub overridden: u32,
    /// Overrides it is the last to have (the game takes them from it).
    pub wins: u32,
    /// Overrides a later plugin takes over whole: a conflict, its changes there are lost unless carried on.
    pub loses: u32,
    /// Overrides equal to the record they override (ITM): still overrides, carrying no change.
    pub unchanged: u32,
}

/// Per plugin (load order), over the whole index.
pub fn plugin_stats(ix: &RecordIndex) -> Vec<PluginRecordStat> {
    let mut out = vec![PluginRecordStat::default(); ix.plugins.len()];
    for e in &ix.entries {
        let s = &mut out[e.plugin as usize];
        s.records += 1;
        if ix.is_origin(e) {
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
            if ix.is_origin(e) {
                s.overridden += 1;
                continue;
            }
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
/// Overridden records with the same plugin stack: one chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordChain {
    /// Plugins in load order, the winner last.
    pub stack: Vec<String>,
    /// Two or more plugins override these records (a conflict); else one plugin overrides its source (an override).
    pub conflict: bool,
    /// A conflict where a later plugin does not have the earlier one as a master (`@sat:records.blind`).
    #[serde(default)]
    pub blind: bool,
    pub records: u32,
    /// Records by signature (`NPC_`, `LVLI`, …).
    pub by_signature: BTreeMap<String, u32>,
    /// Records where every copy is the winner's (nothing is lost, still a conflict).
    pub equal_to_winner: u32,
    /// Records with a copy whose content is not known.
    pub unknown: u32,
    /// A few of its records (`Skyrim.esm:012E49`).
    pub samples: Vec<String>,
    /// Its records (keys of the index) and its stack as load-order indices — for tables, not for the wire.
    #[serde(skip)]
    pub keys: Vec<u64>,
    #[serde(skip)]
    pub stack_ids: Vec<u32>,
}

/// Chains: blind conflicts first, then conflicts on purpose, then overrides, each largest first.
pub fn chains(ix: &RecordIndex) -> Vec<RecordChain> {
    let mut by_stack: HashMap<(Vec<u32>, bool), RecordChain> = HashMap::new();
    for &g in &ix.contested {
        let es = ix.group(g);
        let stack: Vec<u32> = es.iter().map(|e| e.plugin).collect();
        let conflict = ix.overriders(es) >= 2;
        let blind = conflict && ix.blind(es);
        let c = by_stack
            .entry((stack.clone(), conflict))
            .or_insert_with(|| RecordChain {
                conflict,
                blind,
                stack: stack.iter().map(|&p| ix.plugins[p as usize].clone()).collect(),
                records: 0,
                by_signature: BTreeMap::new(),
                equal_to_winner: 0,
                unknown: 0,
                samples: Vec::new(),
                keys: Vec::new(),
                stack_ids: stack.clone(),
            });
        let winner = es[es.len() - 1];
        c.records += 1;
        *c.by_signature.entry(sig(winner.signature)).or_default() += 1;
        if es.iter().any(|e| e.hash.is_none()) {
            c.unknown += 1;
        } else if es.iter().all(|e| e.hash == winner.hash) {
            c.equal_to_winner += 1;
        }
        c.keys.push(winner.key);
        if c.samples.len() < 5 {
            c.samples.push(ix.label(winner.key));
        }
    }
    let mut v: Vec<RecordChain> = by_stack.into_values().collect();
    v.sort_by(|a, b| {
        b.conflict
            .cmp(&a.conflict)
            .then(b.blind.cmp(&a.blind))
            .then(b.records.cmp(&a.records))
            .then_with(|| a.stack.cmp(&b.stack))
    });
    v
}

// @sat:records.contexts
/// Plugins tied by record conflicts (two of them override the same record), with their conflict chains: the record
/// counterpart of the file contexts. A record's own plugin does not tie (else `Skyrim.esm` would tie everything).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordContext {
    /// Load-order indices of the plugins in its chains (own plugins of the records too), in load order.
    pub plugins: Vec<u32>,
    /// Indices into the chains it was built from, in their order.
    pub chains: Vec<usize>,
    pub records: u32,
    pub blind_records: u32,
}

/// Contexts of the conflict chains, largest first.
pub fn contexts(ix: &RecordIndex, chains: &[RecordChain]) -> Vec<RecordContext> {
    let n = ix.plugins.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], mut x: usize) -> usize {
        while p[x] != x {
            p[x] = p[p[x]];
            x = p[x];
        }
        x
    }
    // tie the overriding plugins of every conflict chain (its first key tells the record's own plugin)
    let overriders = |c: &RecordChain| -> Vec<u32> {
        let own = ix.providers(c.keys[0]);
        c.stack_ids
            .iter()
            .copied()
            .filter(|p| !own.iter().any(|x| x.plugin == *p && x.origin))
            .collect()
    };
    let conflict: Vec<usize> = (0..chains.len()).filter(|&i| chains[i].conflict).collect();
    for &i in &conflict {
        let o = overriders(&chains[i]);
        for w in o.windows(2) {
            let (a, b) = (find(&mut parent, w[0] as usize), find(&mut parent, w[1] as usize));
            if a != b {
                parent[a] = b;
            }
        }
    }
    let mut by_root: HashMap<usize, RecordContext> = HashMap::new();
    for &i in &conflict {
        let c = &chains[i];
        let root = find(&mut parent, overriders(c)[0] as usize);
        let cx = by_root.entry(root).or_insert_with(|| RecordContext {
            plugins: Vec::new(),
            chains: Vec::new(),
            records: 0,
            blind_records: 0,
        });
        cx.chains.push(i);
        cx.records += c.records;
        if c.blind {
            cx.blind_records += c.records;
        }
        cx.plugins.extend(&c.stack_ids);
    }
    let mut v: Vec<RecordContext> = by_root
        .into_values()
        .map(|mut c| {
            c.plugins.sort_unstable();
            c.plugins.dedup();
            c
        })
        .collect();
    v.sort_by(|a, b| b.records.cmp(&a.records).then(a.plugins.cmp(&b.plugins)));
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
    fn a_plugin_that_does_not_know_the_earlier_one_overrides_it_blind() {
        let npc = |h| PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(0x0001_2E49, b"NPC_", h)],
        };
        let base = PluginRecords {
            masters: vec![],
            records: vec![rec(0x0001_2E49, b"NPC_", 1)],
        };
        let (a, b) = (npc(2), npc(3));
        let ix = RecordIndex::build(&[
            ("Skyrim.esm".into(), &base),
            ("A.esp".into(), &a),
            ("B.esp".into(), &b),
        ]);
        let ch = chains(&ix);
        assert!(ch[0].conflict && ch[0].blind, "B does not have A as a master");
        let cx = contexts(&ix, &ch);
        assert_eq!(cx.len(), 1);
        assert_eq!(
            (cx[0].plugins.clone(), cx[0].records, cx[0].blind_records),
            (vec![0, 1, 2], 1, 1)
        );
        let pr = ix.providers(ch[0].keys[0]);
        assert_eq!(
            pr.iter().map(|p| (p.plugin, p.origin)).collect::<Vec<_>>(),
            [(0, true), (1, false), (2, false)]
        );
    }

    #[test]
    fn a_record_s_own_plugin_does_not_tie_contexts() {
        let base = PluginRecords {
            masters: vec![],
            records: vec![rec(1, b"NPC_", 1), rec(2, b"NPC_", 1)],
        };
        let over = |id, h| PluginRecords {
            masters: vec!["Skyrim.esm".into()],
            records: vec![rec(id, b"NPC_", h)],
        };
        let (a, b, c, d) = (over(1, 2), over(1, 3), over(2, 4), over(2, 5));
        let ix = RecordIndex::build(&[
            ("Skyrim.esm".into(), &base),
            ("A.esp".into(), &a),
            ("B.esp".into(), &b),
            ("C.esp".into(), &c),
            ("D.esp".into(), &d),
        ]);
        let cx = contexts(&ix, &chains(&ix));
        assert_eq!(
            cx.len(),
            2,
            "A+B and C+D: both through Skyrim records, not tied by it"
        );
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
        assert_eq!(ix.overridden(), 3, "NPC (3 plugins), WEAP (2), A's LVLI (2)");
        assert_eq!(ix.conflicts(), 1, "only the NPC is overridden by two plugins");
        let st = plugin_stats(&ix);
        assert_eq!(
            (st[0].overridden, st[0].loses),
            (2, 0),
            "vanilla is overridden, it does not lose"
        );
        assert_eq!((st[1].new_records, st[1].overrides, st[1].overridden), (1, 2, 1));
        assert_eq!(
            (st[1].wins, st[1].loses, st[1].unchanged),
            (1, 1, 1),
            "A wins WEAP unchanged, loses the NPC"
        );
        assert_eq!(
            (st[2].wins, st[2].loses, st[2].unchanged),
            (2, 0, 1),
            "B's LVLI equals A's"
        );
        let ch = chains(&ix);
        assert_eq!(ch.len(), 3);
        assert!(ch[0].conflict && !ch[1].conflict, "conflicts first");
        assert!(!ch[0].blind, "B has A as a master: on purpose");
        let npc = ch.iter().find(|c| c.stack.len() == 3).unwrap();
        assert_eq!(npc.by_signature["NPC_"], 1);
        assert_eq!(npc.samples, ["skyrim.esm:012E49"]);
        let lvli = ch.iter().find(|c| c.stack == ["A.esp", "B.esp"]).unwrap();
        assert_eq!(lvli.equal_to_winner, 1, "equal, still a conflict");
    }
}
