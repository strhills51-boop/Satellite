//! Sorting the plugin load order by knowledge and facts, in the way LOOT does. @sat:plugins.sort
//!
//! Owns: reading sorting rules from a LOOT masterlist or userlist (groups and their order, a plugin's group, "load
//! after" rules), and the sort itself: a topological order of the active plugins over
//! - the game's rules: base masters first, masters before regular plugins, a plugin after its masters;
//! - rules (the userlist's before the masterlist's): a plugin after the plugins it should load after;
//! - groups: an earlier group's plugins first when nothing else decides;
//! - overlap: of two plugins of one group overriding the same records, the one with more records first — the
//!   smaller, more specific one wins the records they share;
//! - the current order where none of these decides, so a sort moves as little as it can.
//!
//! A rule that would close a cycle with stronger ones is skipped and reported. Inactive plugins keep their order
//! after the active ones. Not here: the facts (`@sat:records`, `@sat:plugins.list`), writing the files
//! (`@sat:plugins.edit`).
//!
//! Conditions of masterlist rules are not evaluated: they mostly keep a rule from crossing the masters' block, which
//! the sort refuses anyway.

use rustc_hash::{FxHashMap, FxHashSet};

use crate::plugins::{PluginList, plugin_key};

/// A plugin's rules: by its name, or by a pattern (masterlist names with `:\*?|` are regular expressions).
#[derive(Debug, Clone)]
pub struct PluginRule {
    pub name: String,
    pub pattern: Option<regex::Regex>,
    pub group: Option<String>,
    pub after: Vec<String>,
}

impl PluginRule {
    fn matches(&self, plugin: &str) -> bool {
        match &self.pattern {
            Some(r) => r.is_match(plugin),
            None => self.name.eq_ignore_ascii_case(plugin),
        }
    }
}

/// Sorting rules of a masterlist or userlist.
#[derive(Debug, Clone, Default)]
pub struct SortRules {
    /// Groups with the groups they load after.
    pub groups: Vec<(String, Vec<String>)>,
    pub plugins: Vec<PluginRule>,
}

fn names(v: Option<&serde_yaml::Value>) -> Vec<String> {
    v.and_then(|v| v.as_sequence())
        .map(|s| {
            s.iter()
                .filter_map(|x| match x {
                    serde_yaml::Value::String(s) => Some(s.clone()),
                    serde_yaml::Value::Mapping(m) => m
                        .get(serde_yaml::Value::from("name"))
                        .and_then(|n| n.as_str())
                        .map(str::to_string),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

impl SortRules {
    // @sat:plugins.sort.rules
    /// Reads the `groups` and `plugins` sections of a LOOT masterlist or userlist (YAML).
    pub fn from_yaml(text: &str) -> Result<Self, String> {
        let doc: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| format!("YAML: {e}"))?;
        let mut rules = Self::default();
        for g in doc
            .get("groups")
            .and_then(|g| g.as_sequence())
            .into_iter()
            .flatten()
        {
            if let Some(name) = g.get("name").and_then(|n| n.as_str()) {
                rules.groups.push((name.to_string(), names(g.get("after"))));
            }
        }
        for p in doc
            .get("plugins")
            .and_then(|p| p.as_sequence())
            .into_iter()
            .flatten()
        {
            let Some(name) = p.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            // templated names of the masterlist's prelude ("{0}") are not plugin names
            if name.contains('{') {
                continue;
            }
            let pattern = if name.contains([':', '\\', '*', '?', '|']) {
                match regex::RegexBuilder::new(&format!("^(?:{name})$"))
                    .case_insensitive(true)
                    .build()
                {
                    Ok(r) => Some(r),
                    Err(_) => continue,
                }
            } else {
                None
            };
            let group = p.get("group").and_then(|g| g.as_str()).map(str::to_string);
            let after = names(p.get("after"));
            if group.is_none() && after.is_empty() {
                continue;
            }
            rules.plugins.push(PluginRule {
                name: name.to_string(),
                pattern,
                group,
                after,
            });
        }
        Ok(rules)
    }

    /// The group of a plugin (the first rule that names one), else LOOT's `default`.
    fn group_of(&self, plugin: &str) -> Option<&str> {
        self.plugins
            .iter()
            .find(|r| r.group.is_some() && r.matches(plugin))
            .and_then(|r| r.group.as_deref())
    }

    /// Plugins this plugin loads after, by every rule that matches it.
    fn after_of(&self, plugin: &str) -> Vec<&str> {
        self.plugins
            .iter()
            .filter(|r| !r.after.is_empty() && r.matches(plugin))
            .flat_map(|r| r.after.iter().map(String::as_str))
            .collect()
    }

    /// Rank of each group: its place in an order of the groups by their `after` lists (unknown groups after
    /// `default`'s neighbours fall back to `default`).
    fn group_ranks(&self) -> FxHashMap<String, usize> {
        let mut rank: FxHashMap<String, usize> = FxHashMap::default();
        let mut left: Vec<&(String, Vec<String>)> = self.groups.iter().collect();
        let mut next = 0;
        while !left.is_empty() {
            let before = left.len();
            left.retain(|(name, after)| {
                if after
                    .iter()
                    .all(|a| rank.contains_key(a) || !self.groups.iter().any(|g| &g.0 == a))
                {
                    rank.insert(name.clone(), next);
                    next += 1;
                    false
                } else {
                    true
                }
            });
            if left.len() == before {
                // a cycle among groups: the rest keep their listed order
                for (name, _) in left.drain(..) {
                    rank.insert(name.clone(), next);
                    next += 1;
                }
            }
        }
        rank
    }
}

/// Why a rule was not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub before: String,
    pub after: String,
    pub rule: &'static str,
}

/// A sorted load order.
#[derive(Debug, Clone, Default)]
pub struct Sorted {
    /// The whole `loadorder.txt`: base masters, the active plugins sorted, the rest in their order.
    pub order: Vec<String>,
    /// Active plugins whose place among the active ones changed.
    pub moved: usize,
    pub skipped: Vec<Skipped>,
    /// Rules kept: (masterlist / userlist "after", overlap).
    pub kept: (usize, usize),
}

/// The plugins' record counts and the pairs that override the same records (by name), from the record index.
pub fn overlaps(ix: &crate::records::RecordIndex) -> (FxHashMap<String, u32>, Vec<(String, String)>) {
    let stats = crate::records::plugin_stats(ix);
    let counts = ix
        .plugins
        .iter()
        .zip(&stats)
        .map(|(n, s)| (plugin_key(n), s.records))
        .collect();
    let mut pairs: FxHashSet<(u32, u32)> = FxHashSet::default();
    for (_, provs) in ix.contested_groups() {
        let over: Vec<u32> = provs.iter().filter(|p| !p.origin).map(|p| p.plugin).collect();
        for (i, &a) in over.iter().enumerate() {
            for &b in &over[i + 1..] {
                if a != b {
                    pairs.insert((a.min(b), a.max(b)));
                }
            }
        }
    }
    let mut out: Vec<(String, String)> = pairs
        .into_iter()
        .map(|(a, b)| (ix.plugins[a as usize].clone(), ix.plugins[b as usize].clone()))
        .collect();
    out.sort();
    (counts, out)
}

/// The order's graph: edges `a → b` (`a` loads before `b`), with in-degrees for Kahn's order.
struct Graph {
    edges: Vec<Vec<usize>>,
    indeg: Vec<u32>,
    seen: Vec<u32>,
    stamp: u32,
}

impl Graph {
    fn new(n: usize) -> Self {
        Self {
            edges: vec![Vec::new(); n],
            indeg: vec![0; n],
            seen: vec![0; n],
            stamp: 0,
        }
    }

    /// Whether `to` is reachable from `from`.
    fn reaches(&mut self, from: usize, to: usize) -> bool {
        self.stamp += 1;
        let mut stack = vec![from];
        while let Some(n) = stack.pop() {
            if n == to {
                return true;
            }
            if self.seen[n] == self.stamp {
                continue;
            }
            self.seen[n] = self.stamp;
            stack.extend(self.edges[n].iter().copied());
        }
        false
    }

    fn push(&mut self, a: usize, b: usize) {
        self.edges[a].push(b);
        self.indeg[b] += 1;
    }
}

// @sat:plugins.sort.run
/// Sorts the active plugins of `list` (see the module). `user` rules go before `master` rules.
pub fn sort(
    list: &PluginList,
    current: &[String],
    master: &SortRules,
    user: &SortRules,
    counts: &FxHashMap<String, u32>,
    overlap: &[(String, String)],
) -> Sorted {
    let base: FxHashSet<String> = crate::plugins::base_plugins()
        .into_iter()
        .map(|(n, _)| plugin_key(&n))
        .collect();
    let pos: FxHashMap<String, usize> = current
        .iter()
        .enumerate()
        .map(|(i, n)| (plugin_key(n), i))
        .collect();
    // the plugins to sort, in their current order
    let mut nodes: Vec<&crate::plugins::Plugin> = list
        .plugins
        .iter()
        .filter(|p| p.active && !base.contains(&plugin_key(&p.name)))
        .collect();
    nodes.sort_by_key(|p| pos.get(&plugin_key(&p.name)).copied().unwrap_or(usize::MAX));
    let at: FxHashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, p)| (plugin_key(&p.name), i))
        .collect();
    let n = nodes.len();
    let ranks = master.group_ranks();
    let default_rank = ranks.get("default").copied().unwrap_or(0);
    let group: Vec<usize> = nodes
        .iter()
        .map(|p| {
            user.group_of(&p.name)
                .or_else(|| master.group_of(&p.name))
                .and_then(|g| ranks.get(g).copied())
                .unwrap_or(default_rank)
        })
        .collect();

    let mut g = Graph::new(n);
    let mut sorted = Sorted::default();
    let masters: Vec<bool> = nodes.iter().map(|p| p.master).collect();
    let name = |i: usize| nodes[i].name.clone();
    // adds `a → b` unless it puts a regular plugin before a master or (checked) closes a cycle
    let add =
        |g: &mut Graph, a: usize, b: usize, rule: &'static str, check: bool, sorted: &mut Sorted| -> bool {
            if a == b || g.edges[a].contains(&b) {
                return true;
            }
            if (!masters[a] && masters[b]) || (check && g.reaches(b, a)) {
                sorted.skipped.push(Skipped {
                    before: name(a),
                    after: name(b),
                    rule,
                });
                return false;
            }
            g.push(a, b);
            true
        };
    // the game's rule: a plugin after its masters
    for (b, p) in nodes.iter().enumerate() {
        for m in &p.masters {
            if let Some(&a) = at.get(&plugin_key(m)) {
                add(&mut g, a, b, "master", false, &mut sorted);
            }
        }
    }
    // load-after rules: the userlist's, then the masterlist's
    for (rules, rule) in [(user, "userlist"), (master, "masterlist")] {
        for (b, p) in nodes.iter().enumerate() {
            for a_name in rules.after_of(&p.name) {
                if let Some(&a) = at.get(&plugin_key(a_name))
                    && add(&mut g, a, b, rule, true, &mut sorted)
                {
                    sorted.kept.0 += 1;
                }
            }
        }
    }
    // overlap inside a group and a block: more records first
    for (x, y) in overlap {
        let (Some(&i), Some(&j)) = (at.get(&plugin_key(x)), at.get(&plugin_key(y))) else {
            continue;
        };
        if group[i] != group[j] || nodes[i].master != nodes[j].master {
            continue;
        }
        let ci = counts.get(&plugin_key(x)).copied().unwrap_or(0);
        let cj = counts.get(&plugin_key(y)).copied().unwrap_or(0);
        let (a, b) = match ci.cmp(&cj) {
            std::cmp::Ordering::Greater => (i, j),
            std::cmp::Ordering::Less => (j, i),
            std::cmp::Ordering::Equal => continue,
        };
        // already ordered one way or the other by stronger rules: nothing to add
        if g.reaches(a, b) {
            continue;
        }
        if add(&mut g, a, b, "overlap", true, &mut sorted) {
            sorted.kept.1 += 1;
        }
    }

    // Kahn's order: masters first, then earlier groups, then the current order
    let Graph { edges, mut indeg, .. } = g;
    let mut ready: std::collections::BTreeSet<(bool, usize, usize)> = (0..n)
        .filter(|&i| indeg[i] == 0)
        .map(|i| (!nodes[i].master, group[i], i))
        .collect();
    let mut out: Vec<usize> = Vec::with_capacity(n);
    while let Some(k) = ready.pop_first() {
        let i = k.2;
        out.push(i);
        for &b in &edges[i] {
            indeg[b] -= 1;
            if indeg[b] == 0 {
                ready.insert((!nodes[b].master, group[b], b));
            }
        }
    }
    // a cycle of masters (the game's own data) cannot be sorted: the rest keep their order
    if out.len() < n {
        let placed: FxHashSet<usize> = out.iter().copied().collect();
        out.extend((0..n).filter(|i| !placed.contains(i)));
    }
    sorted.moved = out.iter().enumerate().filter(|(k, i)| *k != **i).count();

    let active: FxHashSet<String> = nodes.iter().map(|p| plugin_key(&p.name)).collect();
    let mut order: Vec<String> = current
        .iter()
        .filter(|n| base.contains(&plugin_key(n)))
        .cloned()
        .collect();
    order.extend(out.iter().map(|&i| nodes[i].name.clone()));
    order.extend(
        current
            .iter()
            .filter(|n| !base.contains(&plugin_key(n)) && !active.contains(&plugin_key(n)))
            .cloned(),
    );
    sorted.order = order;
    sorted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overwrite::Priorities;
    use crate::plugins::{PluginCanon, PluginVersions};

    fn plugin_mod(name: &str, plugin: &str, masters: &[&str]) -> crate::facts::ModFacts {
        let mut m = crate::table::tests::mk(name, &[]);
        m.plugins.push(crate::facts::PluginFacts {
            file_name: plugin.into(),
            header: Some(sat_formats::PluginHeader {
                masters: masters.iter().map(|s| s.to_string()).collect(),
                version: 1.71,
                ..Default::default()
            }),
            records: None,
            error: None,
        });
        m
    }

    #[test]
    fn masters_rules_groups_and_overlap_order_the_plugins_and_the_rest_stays() {
        let facts = [
            plugin_mod("Small", "Small.esp", &["Skyrim.esm"]),
            plugin_mod("Big", "Big.esp", &["Skyrim.esm"]),
            plugin_mod("Patch", "Patch.esp", &["Small.esp"]),
            plugin_mod("Late", "Late.esp", &["Skyrim.esm"]),
            plugin_mod("Master", "Lib.esm", &["Skyrim.esm"]),
            plugin_mod("After", "After.esp", &["Skyrim.esm"]),
        ];
        let prio = Priorities((0..facts.len() as i32).collect());
        // the current order: everything in the wrong place
        let current: Vec<String> = [
            "Skyrim.esm",
            "Late.esp",
            "Patch.esp",
            "Small.esp",
            "After.esp",
            "Big.esp",
            "Lib.esm",
        ]
        .map(String::from)
        .to_vec();
        let canon = PluginCanon {
            load_order: current.clone(),
            checked: current.iter().skip(1).map(|n| (n.clone(), true)).collect(),
            blacklist: Vec::new(),
        };
        let list = PluginList::compute(&PluginVersions::build(&facts), &canon, &prio);
        let master = SortRules::from_yaml(
            "groups:\n  - name: default\n  - name: Late Loaders\n    after: [default]\n\
             plugins:\n  - name: Late.esp\n    group: Late Loaders\n  - name: 'Aft.*\\.esp'\n    after: [Big.esp]\n",
        )
        .unwrap();
        let counts: FxHashMap<String, u32> = [("small.esp", 5), ("big.esp", 500)]
            .map(|(n, c)| (n.to_string(), c))
            .into_iter()
            .collect();
        let overlap = vec![("Small.esp".to_string(), "Big.esp".to_string())];
        let s = sort(&list, &current, &master, &SortRules::default(), &counts, &overlap);
        let at = |n: &str| s.order.iter().position(|x| x == n).unwrap();
        assert_eq!(s.order[0], "Skyrim.esm", "base masters stay first");
        assert!(
            at("Lib.esm") < at("Small.esp"),
            "a master before regular plugins: {:?}",
            s.order
        );
        assert!(
            at("Big.esp") < at("Small.esp"),
            "more records first: {:?}",
            s.order
        );
        assert!(at("Small.esp") < at("Patch.esp"), "a plugin after its master");
        assert!(at("Big.esp") < at("After.esp"), "a masterlist rule by pattern");
        assert_eq!(
            s.order.last().map(String::as_str),
            Some("Late.esp"),
            "a later group last"
        );
        assert_eq!(s.kept, (1, 1));
        // a rule against the masters' block is refused
        let bad = SortRules::from_yaml("plugins:\n  - name: Lib.esm\n    after: [Big.esp]\n").unwrap();
        let s = sort(&list, &current, &SortRules::default(), &bad, &counts, &[]);
        assert_eq!(s.skipped.len(), 1);
        assert!(s.order.iter().position(|x| x == "Lib.esm") < s.order.iter().position(|x| x == "Big.esp"));
    }
}
