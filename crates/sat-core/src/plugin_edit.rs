//! Editing the plugin canon: check / uncheck, hide, cascade by masters, and the manager's files. @sat:plugins.edit
//!
//! Owns: changes of [`PluginCanon`] the user asks for, and what `plugins.txt` / `loadorder.txt` receive at a commit.
//! Not here: the list projection (`@sat:plugins.list`), writing the files (`sat-mo2::profile`, `@sat:mo2.commit`).
//!
//! Monolith reference: `togglePluginEnabledByName`, `enablePluginsCascadeByDependencies`, `buildPluginCommitFiles`,
//! `ensurePluginBlacklist` (area 14). Kept by meaning; changed:
//! - the files keep the format the manager wrote (star or list) instead of a fixed header and star format;
//! - plugins of disabled mods and hidden plugins are left out (MO2 drops them too), base masters are left out of
//!   `plugins.txt` (the game loads them implicitly) but keep their place in `loadorder.txt`;
//! - active plugins beyond the index limits are reported, not written silently: the game would not load the build.

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::plugins::{Origin, PluginCanon, PluginList, RuntimeIndex, plugin_key};

impl PluginCanon {
    /// Checks or unchecks a plugin (its `plugins.txt` mark). Returns whether anything changed.
    pub fn set_checked(&mut self, name: &str, checked: bool) -> bool {
        let k = plugin_key(name);
        match self.checked.iter_mut().find(|(n, _)| plugin_key(n) == k) {
            Some((_, c)) if *c == checked => false,
            Some((_, c)) => {
                *c = checked;
                true
            }
            None => {
                self.checked.push((name.to_string(), checked));
                true
            }
        }
    }

    /// Hides or shows a plugin. Returns whether anything changed.
    pub fn set_blacklisted(&mut self, name: &str, hidden: bool) -> bool {
        let k = plugin_key(name);
        let at = self.blacklist.iter().position(|n| plugin_key(n) == k);
        match (at, hidden) {
            (Some(_), true) | (None, false) => false,
            (None, true) => {
                self.blacklist.push(name.to_string());
                self.blacklist.sort_by_key(|n| plugin_key(n));
                true
            }
            (Some(i), false) => {
                self.blacklist.remove(i);
                true
            }
        }
    }
}

// @sat:plugins.edit.cascade
/// Checks plugins in layers by masters (monolith "Каскадно включить"): base masters are on; a plugin turns on
/// when it is present, not hidden, and every master is on; repeated until nothing changes. Every other present
/// plugin is unchecked. Plugins of disabled mods and missing plugins keep their mark.
pub fn cascade(list: &PluginList, canon: &PluginCanon) -> PluginCanon {
    let mut on: FxHashSet<String> = list
        .plugins
        .iter()
        .filter(|p| p.origin == Origin::Base)
        .map(|p| plugin_key(&p.name))
        .collect();
    let candidates: Vec<_> = list
        .plugins
        .iter()
        .filter(|p| p.origin != Origin::Base && p.present && !p.blacklisted)
        .collect();
    loop {
        let mut changed = false;
        for p in &candidates {
            let k = plugin_key(&p.name);
            if !on.contains(&k) && p.masters.iter().all(|m| on.contains(&plugin_key(m))) {
                on.insert(k);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    let mut out = canon.clone();
    for p in candidates {
        out.set_checked(&p.name, on.contains(&plugin_key(&p.name)));
    }
    out
}

/// What the manager's plugin files receive.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagerFiles {
    /// `plugins.txt` lines, in load order: name and checked.
    pub plugins: Vec<(String, bool)>,
    /// `loadorder.txt`, in load order.
    pub loadorder: Vec<String>,
    /// Active plugins beyond the index limits: the files must not be written while this is not zero.
    pub overflow: usize,
}

// @sat:plugins.edit.files
/// The manager's files of a plugin list (its order is the load order).
pub fn manager_files(list: &PluginList) -> ManagerFiles {
    let mut f = ManagerFiles::default();
    for p in &list.plugins {
        if p.blacklisted || !p.present {
            continue;
        }
        f.loadorder.push(p.name.clone());
        if p.origin != Origin::Base {
            f.plugins.push((p.name.clone(), p.active));
        }
        f.overflow += usize::from(p.index.is_some_and(RuntimeIndex::overflow));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{ModFacts, PluginFacts};
    use crate::overwrite::Priorities;
    use crate::plugins::{PluginVersions, SKYRIM_BASE};
    use sat_formats::PluginHeader;

    fn plugin(name: &str, masters: &[&str]) -> PluginFacts {
        PluginFacts {
            file_name: name.into(),
            header: Some(PluginHeader {
                masters: masters.iter().map(|s| s.to_string()).collect(),
                ..PluginHeader::default()
            }),
            records: None,
            error: None,
        }
    }

    /// Mod `On` (enabled) has A (no masters), B (needs A), C (needs Missing.esm), H (hidden);
    /// mod `Off` (disabled) has D.
    fn setup(canon: &PluginCanon) -> PluginList {
        let mods = [
            ModFacts {
                name: "On".into(),
                plugins: vec![
                    plugin("A.esp", &["Skyrim.esm"]),
                    plugin("B.esp", &["A.esp"]),
                    plugin("C.esp", &["Missing.esm"]),
                    plugin("H.esp", &[]),
                ],
                ..ModFacts::default()
            },
            ModFacts {
                name: "Off".into(),
                plugins: vec![plugin("D.esp", &[])],
                ..ModFacts::default()
            },
        ];
        PluginList::compute(&PluginVersions::build(&mods), canon, &Priorities(vec![0, -1]))
    }

    #[test]
    fn cascade_turns_plugins_on_in_layers_by_masters() {
        let mut canon = PluginCanon {
            load_order: SKYRIM_BASE.iter().map(|(n, _)| n.to_string()).collect(),
            ..PluginCanon::default()
        };
        canon.set_blacklisted("H.esp", true);
        let list = setup(&canon);
        let c = cascade(&list, &canon);
        let mark = |n: &str| c.checked.iter().find(|(x, _)| x == n).map(|(_, m)| *m);
        assert_eq!(mark("A.esp"), Some(true));
        assert_eq!(mark("B.esp"), Some(true), "its master came on in the first layer");
        assert_eq!(mark("C.esp"), Some(false), "a master is missing");
        assert_eq!(mark("H.esp"), None, "hidden plugins keep their mark");
        assert_eq!(mark("D.esp"), None, "plugins of disabled mods keep their mark");
    }

    #[test]
    fn hidden_plugins_neither_load_nor_count_as_missing_masters() {
        let mut canon = PluginCanon::default();
        canon.set_checked("A.esp", true);
        canon.set_checked("B.esp", true);
        let l = setup(&canon);
        assert!(l.get("A.esp").unwrap().active && l.get("B.esp").unwrap().missing_masters.is_empty());
        canon.set_blacklisted("A.esp", true);
        let l = setup(&canon);
        let (a, b) = (l.get("A.esp").unwrap(), l.get("B.esp").unwrap());
        assert!(a.blacklisted && !a.active && a.index.is_none());
        assert!(
            b.missing_masters.is_empty(),
            "a hidden master is not reported missing"
        );
        assert_eq!(l.stats.blacklisted, 1);
        assert!(canon.set_blacklisted("A.esp", false) && canon.blacklist.is_empty());
    }

    #[test]
    fn manager_files_leave_out_hidden_off_and_base_lines() {
        let mut canon = PluginCanon {
            load_order: vec!["Skyrim.esm".into(), "B.esp".into(), "A.esp".into()],
            ..PluginCanon::default()
        };
        canon.set_checked("A.esp", true);
        canon.set_checked("B.esp", false);
        canon.set_blacklisted("H.esp", true);
        let f = manager_files(&setup(&canon));
        let at = |n: &str| f.loadorder.iter().position(|x| x == n).unwrap();
        assert_eq!(f.loadorder[0], "Skyrim.esm", "base masters first");
        assert!(at("B.esp") < at("A.esp"), "then loadorder.txt");
        assert!(!f.loadorder.iter().any(|n| n == "H.esp" || n == "D.esp"));
        assert!(
            !f.plugins.iter().any(|(n, _)| n.ends_with(".esm")),
            "base masters load implicitly"
        );
        assert!(f.plugins.contains(&("A.esp".into(), true)) && f.plugins.contains(&("B.esp".into(), false)));
        assert_eq!(f.overflow, 0);
    }
}
