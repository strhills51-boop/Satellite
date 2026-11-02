//! Plugins: the load list as the game will see it, from facts, the plugin canon and mod priorities. @sat:plugins
//!
//! Owns: the plugin canon imported from the manager (load order + checked flags), the facts-only index of plugin
//! versions (one plugin name provided by several mods), and the plugin list projection: winning version, state,
//! runtime index (`00`…`FD`, `FE:000`…`FFF`), overflow, missing masters, colour.
//! Not here: reading headers (`@sat:formats.plugin.kind`), finding plugins in a mod (`sat-scan` analyzer
//! `plugins`), mod priorities (`@sat:canon.priorities`).
//!
//! Parity: monolith CP9 + plugin meta owner V527 (`buildPluginMetaIndexFresh` 75 702, `applyPluginOverwriteVersions`
//! 76 091, `calculateMo2PluginIndexes` 75 594, colours 76 997). The monolith's "versionality trap" is solved here
//! by construction: a plugin name is a **group of versions**; facts (masters, flags) always come from the version
//! the VFS would serve — the highest-priority enabled owner — and follow every move or toggle of a mod.
//!
//! Owner's rule (2026-10-31): plugins of disabled mods are full plugins, just off. They keep their header facts
//! and their place in the list; they do not get a runtime index, and dependents see them as missing masters.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use sat_formats::PluginKind;

use crate::facts::{ModFacts, PluginFacts};
use crate::ids::ModId;
use crate::overwrite::Priorities;

/// Folded plugin name (the game and MO2 compare case-insensitively).
pub fn plugin_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Plugins the game itself provides, in their fixed load order (Skyrim SE/AE; a game pack later).
pub const SKYRIM_BASE: &[(&str, &[&str])] = &[
    ("Skyrim.esm", &[]),
    ("Update.esm", &["Skyrim.esm"]),
    ("Dawnguard.esm", &["Skyrim.esm", "Update.esm"]),
    ("HearthFires.esm", &["Skyrim.esm", "Update.esm"]),
    ("Dragonborn.esm", &["Skyrim.esm", "Update.esm"]),
];

/// Creation Club content: shipped in the game's Data, listed in `loadorder.txt`, absent from `plugins.txt`.
pub fn is_creation_club(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let Some((stem, ext)) = n.rsplit_once('.') else {
        return false;
    };
    matches!(ext, "esl" | "esm" | "esp")
        && stem.len() > 2
        && stem.starts_with("cc")
        && stem
            .bytes()
            .skip(2)
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

// @sat:plugins.canon
/// The plugin canon as the manager stores it (MO2: `loadorder.txt` + `plugins.txt`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCanon {
    /// `loadorder.txt`, in order.
    pub load_order: Vec<String>,
    /// `plugins.txt`: name → checked (`*`).
    pub checked: Vec<(String, bool)>,
    /// Plugins the user hid (Satellite's own setting, monolith `analysisConfig.pluginBlacklist`): never loaded,
    /// never written to the manager's lists, and not reported as a missing master of others.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blacklist: Vec<String>,
}

/// What the manager says about a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Listed {
    /// Checked in `plugins.txt`.
    Enabled,
    /// Unchecked in `plugins.txt`.
    Disabled,
    /// In `loadorder.txt` only (base game, Creation Club): loads without a check mark.
    Implicit,
    /// Found in a mod, absent from the manager's lists (changed since the manager last ran).
    Unlisted,
}

/// Where a plugin comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    /// The game's own masters.
    Base,
    /// Creation Club (game Data folder).
    CreationClub,
    /// A mod folder.
    Mod,
    /// Named by the manager's lists or by a master record, provided by nobody.
    Missing,
}

/// Diagnostic colour (monolith semantics).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Colour {
    /// Cannot be used now: missing, or every owner mod is disabled.
    Black,
    /// A master is missing or unusable.
    Red,
    /// Beyond the index limit.
    Yellow,
    /// Found by the scan, not loaded (informational).
    White,
    /// The game's own masters.
    Blue,
    Green,
}

/// One version of a plugin: the copy in one mod.
#[derive(Debug, Clone, PartialEq)]
pub struct Version {
    pub owner: ModId,
    pub kind: PluginKind,
    /// Loads with the masters, before every regular plugin: the ESM flag, or a `.esm` / `.esl` file (a light-flagged
    /// `.esp` is light, not a master).
    pub master: bool,
    pub masters: Vec<String>,
    /// Header could not be read (masters unknown).
    pub error: Option<String>,
}

// @sat:plugins.versions
/// Facts only: plugin name → its versions (one per mod providing it at the mod root). Rebuilt with the facts.
#[derive(Debug, Clone, Default)]
pub struct PluginVersions {
    names: Vec<String>,
    by_key: FxHashMap<String, u32>,
    versions: Vec<Vec<Version>>,
}

fn version_of(owner: ModId, p: &PluginFacts) -> Version {
    Version {
        owner,
        kind: p
            .header
            .as_ref()
            .map_or_else(|| kind_from_name(&p.file_name), |h| h.kind(&p.file_name)),
        master: is_master_file(&p.file_name) || p.header.as_ref().is_some_and(|h| h.master_flag),
        masters: p.header.as_ref().map(|h| h.masters.clone()).unwrap_or_default(),
        error: p.error.clone(),
    }
}

fn is_master_file(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".esm") || l.ends_with(".esl")
}

fn kind_from_name(name: &str) -> PluginKind {
    let l = name.to_ascii_lowercase();
    if l.ends_with(".esl") {
        PluginKind::Esl
    } else if l.ends_with(".esm") {
        PluginKind::Esm
    } else {
        PluginKind::Esp
    }
}

impl PluginVersions {
    /// From scanned mods; `ModId` = position in `mods` (the same order the provider table was built in).
    pub fn build(mods: &[ModFacts]) -> Self {
        let mut me = Self::default();
        for (i, m) in mods.iter().enumerate() {
            me.add(ModId::from_idx(i), m);
        }
        me
    }

    /// Adds one mod's plugins (streaming: facts are folded batch by batch, like the provider table).
    pub fn add(&mut self, owner: ModId, m: &ModFacts) {
        for p in &m.plugins {
            let key = plugin_key(&p.file_name);
            let id = *self.by_key.entry(key).or_insert_with(|| {
                self.names.push(p.file_name.clone());
                self.versions.push(Vec::new());
                (self.names.len() - 1) as u32
            });
            self.versions[id as usize].push(version_of(owner, p));
        }
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

/// One plugin of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct Plugin {
    pub name: String,
    pub origin: Origin,
    pub listed: Listed,
    /// Position in `loadorder.txt`.
    pub load_order: Option<usize>,
    /// The version the game would load (index into [`PluginList::versions`]), if any mod provides it.
    pub winner: Option<usize>,
    /// Every version, winner included, lowest priority first.
    pub versions: Vec<Version>,
    pub kind: PluginKind,
    /// Loads in the masters' block (see [`Version::master`]).
    pub master: bool,
    pub masters: Vec<String>,
    /// At least one owner mod is enabled (or the game provides it).
    pub present: bool,
    /// Loaded by the game.
    pub active: bool,
    /// `00`…`FD` for full plugins, `FE:000`…`FFF` for light ones.
    pub index: Option<RuntimeIndex>,
    /// Hidden by the user (`PluginCanon::blacklist`).
    pub blacklisted: bool,
    pub missing_masters: Vec<String>,
    pub colour: Colour,
}

/// Runtime index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeIndex {
    Full(u32),
    Light(u32),
}

impl RuntimeIndex {
    pub fn overflow(self) -> bool {
        match self {
            RuntimeIndex::Full(i) => i > 0xFD,
            RuntimeIndex::Light(i) => i > 0xFFF,
        }
    }
}

impl std::fmt::Display for RuntimeIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            RuntimeIndex::Full(i) => write!(f, "{i:02X}"),
            RuntimeIndex::Light(i) => write!(f, "FE:{i:03X}"),
        }
    }
}

/// Totals.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginStats {
    pub plugins: usize,
    pub active_full: usize,
    pub active_light: usize,
    pub overflow_full: usize,
    pub overflow_light: usize,
    /// Plugin names provided by more than one mod.
    pub versioned: usize,
    pub with_missing_masters: usize,
    /// Present in a mod but every owner is disabled.
    pub owner_disabled: usize,
    pub missing: usize,
    #[serde(default)]
    pub blacklisted: usize,
}

// @sat:plugins.list
/// The plugin list projection, in load order: `loadorder.txt` first, then plugins it does not list by owner
/// priority and name (monolith `comparePluginLoadPrimitive`).
#[derive(Debug, Clone, Default)]
pub struct PluginList {
    pub plugins: Vec<Plugin>,
    pub stats: PluginStats,
}

impl PluginList {
    pub fn compute(versions: &PluginVersions, canon: &PluginCanon, prio: &Priorities) -> Self {
        let mut checked: FxHashMap<String, bool> = FxHashMap::default();
        for (n, c) in &canon.checked {
            checked.insert(plugin_key(n), *c);
        }
        let mut order: FxHashMap<String, usize> = FxHashMap::default();
        for (i, n) in canon.load_order.iter().enumerate() {
            order.entry(plugin_key(n)).or_insert(i);
        }
        let base: FxHashMap<String, &[&str]> = SKYRIM_BASE.iter().map(|(n, m)| (plugin_key(n), *m)).collect();
        let hidden: rustc_hash::FxHashSet<String> = canon.blacklist.iter().map(|n| plugin_key(n)).collect();

        // every name anybody mentions: base, the manager's lists, mods
        let mut names: Vec<String> = Vec::new();
        let mut seen: FxHashMap<String, usize> = FxHashMap::default();
        let mut add = |n: &str| {
            let k = plugin_key(n);
            if !k.is_empty() && !seen.contains_key(&k) {
                seen.insert(k, names.len());
                names.push(n.trim().to_string());
            }
        };
        for (n, _) in SKYRIM_BASE {
            add(n);
        }
        canon.load_order.iter().for_each(|n| add(n));
        canon.checked.iter().for_each(|(n, _)| add(n));
        versions.names.iter().for_each(|n| add(n));

        let mut plugins: Vec<Plugin> = names
            .into_iter()
            .map(|name| {
                let key = plugin_key(&name);
                let mut vs: Vec<Version> = versions
                    .by_key
                    .get(&key)
                    .map(|&i| versions.versions[i as usize].clone())
                    .unwrap_or_default();
                // lowest priority first; disabled owners count below enabled ones
                vs.sort_by_key(|v| (prio.is_active(v.owner), prio.get(v.owner), v.owner));
                let enabled_owner = vs.last().is_some_and(|v| prio.is_active(v.owner));
                let winner = (!vs.is_empty()).then(|| vs.len() - 1);
                let origin = if base.contains_key(&key) {
                    Origin::Base
                } else if !vs.is_empty() {
                    Origin::Mod
                } else if is_creation_club(&name) && order.contains_key(&key) {
                    Origin::CreationClub
                } else {
                    Origin::Missing
                };
                let listed = match (checked.get(&key), order.contains_key(&key)) {
                    (Some(true), _) => Listed::Enabled,
                    (Some(false), _) => Listed::Disabled,
                    (None, true) => Listed::Implicit,
                    (None, false) if origin == Origin::Base => Listed::Implicit,
                    (None, false) => Listed::Unlisted,
                };
                let (kind, master, masters) = match (winner, origin) {
                    (Some(w), _) => (vs[w].kind, vs[w].master, vs[w].masters.clone()),
                    (None, Origin::Base) => (
                        PluginKind::Esm,
                        true,
                        base[&key].iter().map(|s| s.to_string()).collect(),
                    ),
                    (None, _) => (kind_from_name(&name), is_master_file(&name), Vec::new()),
                };
                let present = matches!(origin, Origin::Base | Origin::CreationClub) || enabled_owner;
                // a base master cannot be hidden: the game loads it whatever the lists say
                let blacklisted = origin != Origin::Base && hidden.contains(&key);
                // base masters always load; otherwise not hidden, not unchecked, present, and known or found
                let active =
                    origin == Origin::Base || (!blacklisted && present && listed != Listed::Disabled);
                Plugin {
                    load_order: order.get(&key).copied(),
                    name,
                    origin,
                    listed,
                    winner,
                    versions: vs,
                    kind,
                    master,
                    masters,
                    present,
                    active,
                    index: None,
                    blacklisted,
                    missing_masters: Vec::new(),
                    colour: Colour::Green,
                }
            })
            .collect();

        // load order: base masters, then loadorder.txt, then the rest by owner priority and name
        let tail_prio = |p: &Plugin| p.winner.map_or(i32::MAX, |w| prio.get(p.versions[w].owner));
        plugins.sort_by(|a, b| {
            let rank = |p: &Plugin| match (p.origin, p.load_order) {
                (Origin::Base, _) => 0,
                (_, Some(_)) => 1,
                _ => 2,
            };
            rank(a)
                .cmp(&rank(b))
                .then_with(|| match (a.origin, b.origin) {
                    (Origin::Base, Origin::Base) => {
                        let pos = |p: &Plugin| {
                            SKYRIM_BASE
                                .iter()
                                .position(|(n, _)| plugin_key(n) == plugin_key(&p.name))
                        };
                        pos(a).cmp(&pos(b))
                    }
                    _ => a.load_order.cmp(&b.load_order),
                })
                .then_with(|| tail_prio(a).cmp(&tail_prio(b)))
                .then_with(|| plugin_key(&a.name).cmp(&plugin_key(&b.name)))
        });

        // runtime indexes in load order, full and light counted apart
        let (mut full, mut light) = (0u32, 0u32);
        let mut st = PluginStats {
            plugins: plugins.len(),
            ..PluginStats::default()
        };
        for p in &mut plugins {
            if !p.active {
                continue;
            }
            p.index = Some(if p.kind == PluginKind::Esl {
                light += 1;
                RuntimeIndex::Light(light - 1)
            } else {
                full += 1;
                RuntimeIndex::Full(full - 1)
            });
        }
        let usable: FxHashMap<String, bool> =
            plugins.iter().map(|p| (plugin_key(&p.name), p.active)).collect();
        for p in &mut plugins {
            p.missing_masters = p
                .masters
                .iter()
                .filter(|m| {
                    !hidden.contains(&plugin_key(m)) && !usable.get(&plugin_key(m)).copied().unwrap_or(false)
                })
                .cloned()
                .collect();
            let overflow = p.index.is_some_and(RuntimeIndex::overflow);
            p.colour = if p.origin == Origin::Base {
                Colour::Blue
            } else if !p.present {
                Colour::Black
            } else if !p.missing_masters.is_empty() {
                Colour::Red
            } else if overflow {
                Colour::Yellow
            } else if !p.active {
                Colour::White
            } else {
                Colour::Green
            };
            match p.index {
                Some(RuntimeIndex::Full(_)) => st.active_full += 1,
                Some(RuntimeIndex::Light(_)) => st.active_light += 1,
                None => {}
            }
            if overflow {
                match p.index {
                    Some(RuntimeIndex::Full(_)) => st.overflow_full += 1,
                    _ => st.overflow_light += 1,
                }
            }
            st.versioned += usize::from(p.versions.len() > 1);
            st.with_missing_masters += usize::from(p.present && !p.missing_masters.is_empty());
            st.owner_disabled += usize::from(p.origin == Origin::Mod && !p.present);
            st.missing += usize::from(p.origin == Origin::Missing);
            st.blacklisted += usize::from(p.blacklisted);
        }
        PluginList { plugins, stats: st }
    }

    pub fn get(&self, name: &str) -> Option<&Plugin> {
        let k = plugin_key(name);
        self.plugins.iter().find(|p| plugin_key(&p.name) == k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sat_formats::PluginHeader;

    fn plugin(name: &str, masters: &[&str], light: bool) -> PluginFacts {
        PluginFacts {
            file_name: name.into(),
            header: Some(PluginHeader {
                light_flag: light,
                masters: masters.iter().map(|s| s.to_string()).collect(),
                ..PluginHeader::default()
            }),
            records: None,
            error: None,
        }
    }

    fn mod_with(name: &str, plugins: Vec<PluginFacts>) -> ModFacts {
        ModFacts {
            name: name.into(),
            plugins,
            ..ModFacts::default()
        }
    }

    fn canon(order: &[&str], checked: &[(&str, bool)]) -> PluginCanon {
        PluginCanon {
            load_order: order.iter().map(|s| s.to_string()).collect(),
            checked: checked.iter().map(|(n, c)| (n.to_string(), *c)).collect(),
            blacklist: Vec::new(),
        }
    }

    #[test]
    fn versions_follow_the_winning_owner() {
        // the same plugin in two mods with different masters: the version served is the higher enabled owner
        let mods = [
            mod_with("Old", vec![plugin("Patch.esp", &["Skyrim.esm"], false)]),
            mod_with(
                "New",
                vec![plugin("Patch.esp", &["Skyrim.esm", "Dawnguard.esm"], false)],
            ),
        ];
        let v = PluginVersions::build(&mods);
        let c = canon(&["Skyrim.esm", "Patch.esp"], &[("Patch.esp", true)]);
        let l = PluginList::compute(&v, &c, &Priorities(vec![0, 1]));
        let p = l.get("patch.esp").unwrap();
        assert_eq!(p.versions[p.winner.unwrap()].owner, ModId(1));
        assert_eq!(p.masters, ["Skyrim.esm", "Dawnguard.esm"]);
        assert_eq!(l.stats.versioned, 1);
        // the newer mod is disabled: the old version is served, facts change with it
        let l = PluginList::compute(&v, &c, &Priorities(vec![0, -1]));
        let p = l.get("patch.esp").unwrap();
        assert_eq!(p.versions[p.winner.unwrap()].owner, ModId(0));
        assert_eq!(p.masters, ["Skyrim.esm"]);
        // the order flips: the old mod is now on top
        let l = PluginList::compute(&v, &c, &Priorities(vec![2, 1]));
        assert_eq!(l.get("patch.esp").unwrap().masters, ["Skyrim.esm"]);
    }

    #[test]
    fn plugins_of_disabled_mods_are_full_plugins_just_off() {
        let mods = [
            mod_with("Master", vec![plugin("Lib.esm", &["Skyrim.esm"], false)]),
            mod_with("User", vec![plugin("User.esp", &["Lib.esm"], false)]),
        ];
        let v = PluginVersions::build(&mods);
        let c = canon(
            &["Skyrim.esm", "Lib.esm", "User.esp"],
            &[("Lib.esm", true), ("User.esp", true)],
        );
        let l = PluginList::compute(&v, &c, &Priorities(vec![-1, 1]));
        let lib = l.get("Lib.esm").unwrap();
        assert_eq!(lib.origin, Origin::Mod);
        assert_eq!(lib.masters, ["Skyrim.esm"], "header facts are kept");
        assert!(!lib.present && !lib.active && lib.index.is_none());
        assert_eq!(lib.colour, Colour::Black);
        let user = l.get("User.esp").unwrap();
        assert_eq!(user.missing_masters, ["Lib.esm"]);
        assert_eq!(user.colour, Colour::Red);
        assert_eq!(l.stats.owner_disabled, 1);
    }

    #[test]
    fn indexes_states_and_limits() {
        let mut ps: Vec<PluginFacts> = (0..260)
            .map(|i| plugin(&format!("Full{i:03}.esp"), &[], false))
            .collect();
        ps.push(plugin("Small.esp", &[], true)); // ESL-flagged esp
        ps.push(plugin("Off.esp", &[], false));
        ps.push(plugin("Fresh.esp", &[], false)); // not in the manager's lists
        let v = PluginVersions::build(&[mod_with("Big", ps)]);
        let mut order: Vec<String> = SKYRIM_BASE.iter().map(|(n, _)| n.to_string()).collect();
        order.push("ccBGSSSE001-Fish.esm".into());
        order.extend((0..260).map(|i| format!("Full{i:03}.esp")));
        order.push("Small.esp".into());
        order.push("Off.esp".into());
        let mut checked: Vec<(String, bool)> = (0..260).map(|i| (format!("Full{i:03}.esp"), true)).collect();
        checked.push(("Small.esp".into(), true));
        checked.push(("Off.esp".into(), false));
        let c = PluginCanon {
            load_order: order,
            checked,
            ..PluginCanon::default()
        };
        let l = PluginList::compute(&v, &c, &Priorities(vec![0]));

        assert_eq!(l.get("Skyrim.esm").unwrap().index, Some(RuntimeIndex::Full(0)));
        assert_eq!(l.get("Skyrim.esm").unwrap().colour, Colour::Blue);
        let cc = l.get("ccbgssse001-fish.esm").unwrap();
        assert_eq!(
            (cc.origin, cc.listed, cc.active),
            (Origin::CreationClub, Listed::Implicit, true)
        );
        assert_eq!(cc.index.unwrap().to_string(), "05");
        let small = l.get("Small.esp").unwrap();
        assert_eq!(small.index.unwrap().to_string(), "FE:000");
        let off = l.get("Off.esp").unwrap();
        assert_eq!(
            (off.listed, off.active, off.colour),
            (Listed::Disabled, false, Colour::White)
        );
        let fresh = l.get("Fresh.esp").unwrap();
        assert_eq!(fresh.listed, Listed::Unlisted);
        assert!(
            fresh.active,
            "a root plugin of an enabled mod loads even if the manager has not listed it yet"
        );
        assert_eq!(
            l.plugins.last().unwrap().name,
            "Fresh.esp",
            "unlisted plugins go after loadorder.txt"
        );
        // 5 base + 1 CC + 260 + Fresh = 267 full; 0xFE.. overflow
        assert_eq!(l.stats.active_full, 267);
        assert_eq!(l.stats.overflow_full, 267 - 0xFE);
        assert_eq!(l.get("Full247.esp").unwrap().index.unwrap().to_string(), "FD");
        assert_eq!(l.get("Full248.esp").unwrap().colour, Colour::Yellow);
        assert_eq!(l.stats.active_light, 1);
    }

    #[test]
    fn missing_plugins_and_masters() {
        let v = PluginVersions::build(&[mod_with("A", vec![plugin("A.esp", &["Gone.esm"], false)])]);
        let c = canon(
            &["Skyrim.esm", "Ghost.esp", "A.esp"],
            &[("Ghost.esp", true), ("A.esp", true)],
        );
        let l = PluginList::compute(&v, &c, &Priorities(vec![0]));
        let g = l.get("Ghost.esp").unwrap();
        assert_eq!(
            (g.origin, g.present, g.colour),
            (Origin::Missing, false, Colour::Black)
        );
        assert_eq!(l.get("A.esp").unwrap().missing_masters, ["Gone.esm"]);
        assert_eq!(l.stats.missing, 1);
    }
}
