//! Editing the plugin canon: check / uncheck, hide, cascade by masters, and the manager's files. @sat:plugins.edit
//!
//! Owns: changes of [`PluginCanon`] the user asks for — marks, hiding, cascade, the load order — and what
//! `plugins.txt` / `loadorder.txt` receive at a commit.
//! Not here: the list projection (`@sat:plugins.list`), writing the files (`sat-mo2::profile`, `@sat:mo2.commit`).
//!
//! Monolith reference: `togglePluginEnabledByName`, `enablePluginsCascadeByDependencies`, `buildPluginCommitFiles`,
//! `ensurePluginBlacklist` (area 14). Kept by meaning; changed:
//! - the files keep the format the manager wrote (star or list) instead of a fixed header and star format;
//! - plugins of disabled mods and hidden plugins are left out (MO2 drops them too), base masters are left out of
//!   `plugins.txt` (the game loads them implicitly) but keep their place in `loadorder.txt`;
//! - active plugins beyond the index limits are reported, not written silently: the game would not load the build;
//! - the monolith only showed the load order; moving plugins is NG's own, with the game's two rules as guards
//!   (masters load before regular plugins, a plugin after its masters) — a move that breaks them anew is refused;
//!   problems the order already had (a hand-made `loadorder.txt`) are not the move's.

use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::plugins::{Origin, Plugin, PluginCanon, PluginList, RuntimeIndex, plugin_key};

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

// @sat:plugins.edit.order
/// Why a plugin move is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PluginOrderError {
    #[error("no plugin {name:?}")]
    Unknown { name: String },
    #[error("{name} is a base master: the game loads it first")]
    Base { name: String },
    #[error("{regular} cannot be placed above {master}: masters load before regular plugins")]
    MasterBelowRegular { master: String, regular: String },
    #[error("{plugin} needs {master}: it cannot load before it")]
    AboveMaster { plugin: String, master: String },
}

impl PluginCanon {
    /// Moves plugins (in their current order) to just before `before`, or to the end of the load order. The whole
    /// order is written into `load_order` (plugins the manager had not listed get their place too). Returns
    /// whether anything changed.
    pub fn move_plugins(
        &mut self,
        list: &PluginList,
        names: &[String],
        before: Option<&str>,
    ) -> Result<bool, PluginOrderError> {
        let moving: FxHashSet<String> = names.iter().map(|n| plugin_key(n)).collect();
        for n in names {
            match list.get(n) {
                None => return Err(PluginOrderError::Unknown { name: n.clone() }),
                Some(p) if p.origin == Origin::Base => {
                    return Err(PluginOrderError::Base { name: p.name.clone() });
                }
                Some(_) => {}
            }
        }
        let (base, rest): (Vec<&Plugin>, Vec<&Plugin>) =
            list.plugins.iter().partition(|p| p.origin == Origin::Base);
        let (picked, mut order): (Vec<&Plugin>, Vec<&Plugin>) = rest
            .into_iter()
            .partition(|p| moving.contains(&plugin_key(&p.name)));
        let at = match before {
            None => order.len(),
            Some(b) => {
                let p = list
                    .get(b)
                    .ok_or_else(|| PluginOrderError::Unknown { name: b.into() })?;
                if p.origin == Origin::Base {
                    return Err(PluginOrderError::Base { name: p.name.clone() });
                }
                match order.iter().position(|x| plugin_key(&x.name) == plugin_key(b)) {
                    Some(i) => i,
                    // the target is one of the moved plugins: they stay where they are
                    None => return Ok(false),
                }
            }
        };
        let old: rustc_hash::FxHashMap<String, usize> = list
            .plugins
            .iter()
            .enumerate()
            .map(|(i, p)| (plugin_key(&p.name), i))
            .collect();
        order.splice(at..at, picked);
        check_order(&order, &moving, &old)?;
        // base masters stay as the file had them (a file without the DLC lines does not get them)
        let listed: FxHashSet<String> = self.load_order.iter().map(|n| plugin_key(n)).collect();
        let new: Vec<String> = base
            .iter()
            .filter(|p| listed.contains(&plugin_key(&p.name)))
            .chain(order.iter())
            .map(|p| p.name.clone())
            .collect();
        if new == self.load_order {
            return Ok(false);
        }
        self.load_order = new;
        Ok(true)
    }
}

/// The game's two order rules, for pairs with a moved plugin in them that the move turned around (`old` — positions
/// before the move): a pair already out of order is not the move's doing.
fn check_order(
    order: &[&Plugin],
    moving: &FxHashSet<String>,
    old: &rustc_hash::FxHashMap<String, usize>,
) -> Result<(), PluginOrderError> {
    let keys: Vec<String> = order.iter().map(|p| plugin_key(&p.name)).collect();
    let was: Vec<usize> = keys
        .iter()
        .map(|k| old.get(k).copied().unwrap_or(usize::MAX))
        .collect();
    let needs = |a: usize, b: usize| order[a].masters.iter().any(|m| plugin_key(m) == keys[b]);
    for i in (0..order.len()).filter(|&i| moving.contains(&keys[i])) {
        for j in (0..order.len()).filter(|&j| j != i) {
            // `a` now above `b`; a pair already in this order is not the move's doing
            let (a, b) = if i < j { (i, j) } else { (j, i) };
            if was[a] < was[b] {
                continue;
            }
            if !order[a].master && order[b].master {
                return Err(PluginOrderError::MasterBelowRegular {
                    master: order[b].name.clone(),
                    regular: order[a].name.clone(),
                });
            }
            if needs(a, b) {
                return Err(PluginOrderError::AboveMaster {
                    plugin: order[a].name.clone(),
                    master: order[b].name.clone(),
                });
            }
        }
    }
    Ok(())
}

// @sat:plugins.edit.reconcile
/// What reconciling the project's plugin canon with the manager's files found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// The project keeps edits of its own (the commit writes them).
    pub pending: bool,
    /// The manager changed the same part too: the project's edit stands, the manager's change is dropped.
    pub conflict: bool,
}

/// The plugin canon after reading the manager's files `disk`: the load order as a whole and each plugin's check mark
/// on its own — the project's version where the user edited it since the manager's files were last `seen`, else
/// the manager's. Hidden plugins are the project's own. Without `seen` (an older project) the manager's files are
/// taken.
pub fn reconcile(
    project: &PluginCanon,
    seen: Option<&PluginCanon>,
    disk: &PluginCanon,
) -> (PluginCanon, Reconciled) {
    let mut r = Reconciled::default();
    let mut part = |mine: bool, theirs: bool| match seen {
        Some(_) if mine => {
            r.conflict |= theirs;
            true
        }
        _ => false,
    };
    let order_edited = seen.is_some_and(|s| s.load_order != project.load_order);
    let order_changed = seen.is_some_and(|s| s.load_order != disk.load_order);
    let keep_order = part(order_edited, order_changed);
    let checked = match seen {
        None => disk.checked.clone(),
        Some(seen) => {
            let map = |c: &PluginCanon| -> rustc_hash::FxHashMap<String, bool> {
                c.checked.iter().map(|(n, m)| (plugin_key(n), *m)).collect()
            };
            let (was, mine, theirs) = (map(seen), map(project), map(disk));
            // the manager's lines in its order with the user's marks where the user changed them, then plugins the
            // user marked that the manager does not list; a plugin the manager dropped stays dropped unless edited
            let mut out: Vec<(String, bool)> = Vec::new();
            let mut placed = FxHashSet::default();
            for (n, m) in &disk.checked {
                let k = plugin_key(n);
                let edited = mine.get(&k).copied().filter(|x| was.get(&k) != Some(x));
                if let Some(x) = edited {
                    r.conflict |= was.get(&k) != Some(m) && *m != x;
                }
                out.push((n.clone(), edited.unwrap_or(*m)));
                placed.insert(k);
            }
            for (n, m) in &project.checked {
                let k = plugin_key(n);
                if !placed.contains(&k) && was.get(&k) != Some(m) && !theirs.contains_key(&k) {
                    out.push((n.clone(), *m));
                    placed.insert(k);
                }
            }
            out
        }
    };
    let out = PluginCanon {
        load_order: if keep_order {
            &project.load_order
        } else {
            &disk.load_order
        }
        .clone(),
        checked,
        blacklist: project.blacklist.clone(),
    };
    r.pending = out.load_order != disk.load_order || out.checked != disk.checked;
    (out, r)
}

// @sat:plugins.edit.canon-files
/// The manager's files straight from a canon, for a commit that has no plugin list at hand: `plugins.txt` lines in
/// load order (base masters and hidden plugins left out, marks not in the load order after it), `loadorder.txt`
/// without hidden plugins.
pub fn canon_files(canon: &PluginCanon) -> (Vec<(String, bool)>, Vec<String>) {
    let hidden: FxHashSet<String> = canon.blacklist.iter().map(|n| plugin_key(n)).collect();
    let base: FxHashSet<String> = crate::plugins::base_plugins()
        .iter()
        .map(|(n, _)| plugin_key(n))
        .collect();
    let marks: rustc_hash::FxHashMap<String, (&str, bool)> = canon
        .checked
        .iter()
        .map(|(n, c)| (plugin_key(n), (n.as_str(), *c)))
        .collect();
    let loadorder: Vec<String> = canon
        .load_order
        .iter()
        .filter(|n| !hidden.contains(&plugin_key(n)))
        .cloned()
        .collect();
    let mut placed = FxHashSet::default();
    let mut plugins = Vec::new();
    for n in &loadorder {
        let k = plugin_key(n);
        if let Some(&(_, c)) = marks.get(&k)
            && !base.contains(&k)
            && placed.insert(k)
        {
            plugins.push((n.clone(), c));
        }
    }
    for (n, c) in &canon.checked {
        let k = plugin_key(n);
        if !hidden.contains(&k) && !base.contains(&k) && placed.insert(k) {
            plugins.push((n.clone(), *c));
        }
    }
    (plugins, loadorder)
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
    use crate::plugins::{PluginVersions, base_plugins};
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
            load_order: base_plugins().into_iter().map(|(n, _)| n).collect(),
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
    fn moving_plugins_keeps_the_games_order_rules() {
        let mods = [ModFacts {
            name: "On".into(),
            plugins: vec![
                plugin("M.esm", &["Skyrim.esm"]),
                plugin("A.esp", &["M.esm"]),
                plugin("B.esp", &["A.esp"]),
                plugin("C.esp", &[]),
            ],
            ..ModFacts::default()
        }];
        let mut canon = PluginCanon {
            load_order: vec![
                "Skyrim.esm".into(),
                "M.esm".into(),
                "A.esp".into(),
                "B.esp".into(),
                "C.esp".into(),
            ],
            ..PluginCanon::default()
        };
        let list =
            |c: &PluginCanon| PluginList::compute(&PluginVersions::build(&mods), c, &Priorities(vec![0]));
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let order = |c: &PluginCanon| c.load_order.iter().skip(1).cloned().collect::<Vec<_>>();
        // C up above A: fine (it needs nothing)
        assert_eq!(
            canon.move_plugins(&list(&canon), &names(&["C.esp"]), Some("A.esp")),
            Ok(true)
        );
        assert_eq!(order(&canon), ["M.esm", "C.esp", "A.esp", "B.esp"]);
        assert_eq!(
            canon.load_order[0], "Skyrim.esm",
            "base first, only the ones the file had"
        );
        // A and B together are already last: nothing changes; C to the end
        assert_eq!(
            canon.move_plugins(&list(&canon), &names(&["B.esp", "A.esp"]), None),
            Ok(false)
        );
        assert_eq!(
            canon.move_plugins(&list(&canon), &names(&["C.esp"]), None),
            Ok(true)
        );
        assert_eq!(order(&canon), ["M.esm", "A.esp", "B.esp", "C.esp"]);
        // refused: B above its master A; a master below a regular plugin; A above its master M; a base master
        let before = canon.clone();
        let l = list(&canon);
        assert!(matches!(
            canon.move_plugins(&l, &names(&["B.esp"]), Some("A.esp")),
            Err(PluginOrderError::AboveMaster { .. })
        ));
        assert!(matches!(
            canon.move_plugins(&l, &names(&["M.esm"]), None),
            Err(PluginOrderError::MasterBelowRegular { .. })
        ));
        assert!(matches!(
            canon.move_plugins(&l, &names(&["C.esp"]), Some("M.esm")),
            Err(PluginOrderError::MasterBelowRegular { .. })
        ));
        assert!(matches!(
            canon.move_plugins(&l, &names(&["Update.esm"]), None),
            Err(PluginOrderError::Base { .. })
        ));
        assert!(matches!(
            canon.move_plugins(&l, &names(&["Nope.esp"]), None),
            Err(PluginOrderError::Unknown { .. })
        ));
        assert_eq!(canon, before, "a refused move changes nothing");
        // the list follows the canon
        assert!(l.get("C.esp").unwrap().load_order > l.get("B.esp").unwrap().load_order);

        // a hand-made order with a regular plugin above a master (X.esm): moves that keep that pair are fine,
        // a move that makes a new such pair is not
        let mods2 = [ModFacts {
            name: "On".into(),
            plugins: vec![plugin("A.esp", &[]), plugin("C.esp", &[]), plugin("X.esm", &[])],
            ..ModFacts::default()
        }];
        let mut hand = PluginCanon {
            load_order: ["A.esp", "C.esp", "X.esm"].map(String::from).to_vec(),
            ..PluginCanon::default()
        };
        let l2 = PluginList::compute(&PluginVersions::build(&mods2), &hand, &Priorities(vec![0]));
        assert_eq!(
            hand.move_plugins(&l2, &names(&["C.esp"]), Some("A.esp")),
            Ok(true)
        );
        let l2 = PluginList::compute(&PluginVersions::build(&mods2), &hand, &Priorities(vec![0]));
        assert_eq!(
            hand.move_plugins(&l2, &names(&["X.esm"]), Some("C.esp")),
            Ok(true),
            "a master up: fine"
        );
        let l2 = PluginList::compute(&PluginVersions::build(&mods2), &hand, &Priorities(vec![0]));
        assert!(matches!(
            hand.move_plugins(&l2, &names(&["C.esp"]), Some("X.esm")),
            Err(PluginOrderError::MasterBelowRegular { .. })
        ));
    }

    #[test]
    fn reconciling_keeps_the_users_edits_and_takes_the_managers() {
        let canon = |order: &[&str], marks: &[(&str, bool)]| PluginCanon {
            load_order: order.iter().map(|s| s.to_string()).collect(),
            checked: marks.iter().map(|(n, c)| (n.to_string(), *c)).collect(),
            blacklist: Vec::new(),
        };
        let seen = canon(&["A.esp", "B.esp"], &[("A.esp", true), ("B.esp", true)]);
        // nothing edited: the manager's change is taken
        let disk = canon(&["B.esp", "A.esp"], &[("A.esp", true), ("B.esp", false)]);
        let (c, r) = reconcile(&seen, Some(&seen), &disk);
        assert_eq!(
            (c.load_order.clone(), c.checked.clone(), r),
            (
                disk.load_order.clone(),
                disk.checked.clone(),
                Reconciled::default()
            )
        );
        // the order edited in the project, marks changed in the manager: both stand
        let mut mine = seen.clone();
        mine.load_order = vec!["B.esp".into(), "A.esp".into()];
        mine.blacklist = vec!["H.esp".into()];
        let disk2 = canon(&["A.esp", "B.esp"], &[("A.esp", false), ("B.esp", true)]);
        let (c, r) = reconcile(&mine, Some(&seen), &disk2);
        assert_eq!(c.load_order, mine.load_order);
        assert_eq!(c.checked, disk2.checked);
        assert_eq!(c.blacklist, ["H.esp"]);
        assert_eq!(
            r,
            Reconciled {
                pending: true,
                conflict: false
            }
        );
        // marks of different plugins changed on both sides: both stand, no conflict
        let mut mine2 = seen.clone();
        mine2.checked = vec![("A.esp".into(), false), ("B.esp".into(), true)];
        let disk3 = canon(&["A.esp", "B.esp"], &[("A.esp", true), ("B.esp", false)]);
        let (c, r) = reconcile(&mine2, Some(&seen), &disk3);
        assert_eq!(
            c.checked,
            [("A.esp".to_string(), false), ("B.esp".to_string(), false)]
        );
        assert!(!r.conflict && r.pending);
        // a mark the user added survives; a plugin only the manager changed takes the manager's mark
        let mut mine3 = seen.clone();
        mine3.checked.push(("N.esp".into(), true));
        let (c, r) = reconcile(&mine3, Some(&seen), &disk3);
        assert_eq!(c.checked[1], ("B.esp".to_string(), false));
        assert_eq!(c.checked[2], ("N.esp".to_string(), true));
        assert!(!r.conflict);
        // both changed the order: the project's stands, reported
        let (c, r) = reconcile(
            &mine,
            Some(&seen),
            &canon(&["A.esp", "C.esp", "B.esp"], &[("A.esp", true), ("B.esp", true)]),
        );
        assert_eq!((c.load_order, r.conflict), (mine.load_order.clone(), true));
        // an older project: the manager's files
        let (c, r) = reconcile(&mine, None, &disk);
        assert_eq!((c.load_order, r.pending), (disk.load_order, false));
    }

    #[test]
    fn canon_files_follow_the_load_order() {
        let canon = PluginCanon {
            load_order: vec![
                "Skyrim.esm".into(),
                "B.esp".into(),
                "H.esp".into(),
                "A.esp".into(),
            ],
            checked: vec![
                ("A.esp".into(), true),
                ("B.esp".into(), false),
                ("H.esp".into(), true),
                ("New.esp".into(), true),
            ],
            blacklist: vec!["H.esp".into()],
        };
        let (plugins, loadorder) = canon_files(&canon);
        assert_eq!(loadorder, ["Skyrim.esm", "B.esp", "A.esp"]);
        assert_eq!(
            plugins,
            [
                ("B.esp".into(), false),
                ("A.esp".into(), true),
                ("New.esp".to_string(), true)
            ]
        );
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
