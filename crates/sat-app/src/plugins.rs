//! The plugin list in the window: one plugin truth, many projections. @sat:app.plugins
//!
//! The projections: the full list, a plugin's card, a mod's plugins, a mod's indicator, the hover cloud.
//! Owns: a plugin as the window shows it ([`PluginView`]: states that stack, caste, index, the copies in mods with
//! the winner, masters with why each is missing, dependents), the full list with the masters nobody has
//! ([`Query::Plugins`](crate::Query::Plugins)), and what a mod's plugins put on its indicator (`RowView::plugins`).
//! Not here: the list itself (`@sat:plugins.list`, states `@sat:plugins.states`, the mod mark
//! `@sat:plugins.mod-mark`), edits of it (`@sat:app.plugins-edit` in `lib.rs`).
//!
//! The monolith built a plugin meta block over its own index (`renderPluginsMetaBlock`, owners V527); here every
//! projection reads the one [`PluginList`] the session keeps, so a toggle of a mod or a plugin shows the same in all
//! of them.

use serde::{Deserialize, Serialize};

use rustc_hash::FxHashMap;
use sat_core::ids::ModId;
use sat_core::order::Key;
use sat_core::plugins::{
    Caste, Colour, Ghost, Listed, MasterGap, Origin, Plugin, PluginList, PluginState, PluginStats, plugin_key,
};

use crate::App;

/// One copy of a plugin: the mod that has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginOwner {
    pub name: String,
    /// The mod's key in the order (absent for the overwrite folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<Key>,
    pub enabled: bool,
    /// The copy the game takes.
    pub winner: bool,
}

/// One plugin as the window shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginView {
    pub name: String,
    pub index: Option<String>,
    pub colour: Colour,
    pub listed: Listed,
    pub active: bool,
    pub owner: Option<String>,
    pub versions: usize,
    pub missing_masters: Vec<String>,
    /// What it does in the record layer (`@sat:records.plugin-stat`), once computed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub records: Option<sat_core::records::PluginRecordStat>,
    /// Every state, priority first (`@sat:plugins.states`): the window stacks them on the plugin's container.
    #[serde(default)]
    pub states: Vec<PluginState>,
    pub caste: Caste,
    pub origin: Origin,
    /// Position in the load order (the list's own order).
    pub order: u32,
    #[serde(default)]
    pub masters: Vec<String>,
    /// Why each of `missing_masters` is missing (same order).
    #[serde(default)]
    pub gaps: Vec<MasterGap>,
    /// Every copy, the lowest priority first (the winner last among enabled ones).
    #[serde(default)]
    pub owners: Vec<PluginOwner>,
    /// Plugins that list it as a master.
    #[serde(default)]
    pub dependents: Vec<String>,
}

/// The whole plugin list: plugins in load order, the masters nobody has, the totals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginsPage {
    pub plugins: Vec<PluginView>,
    pub ghosts: Vec<Ghost>,
    pub stats: PluginStats,
}

/// Plugin name → the plugins that list it as a master.
pub(crate) fn dependents(list: &PluginList) -> FxHashMap<String, Vec<String>> {
    let mut out: FxHashMap<String, Vec<String>> = FxHashMap::default();
    for p in &list.plugins {
        for m in &p.masters {
            out.entry(plugin_key(m)).or_default().push(p.name.clone());
        }
    }
    out
}

impl App {
    /// A plugin of the session's list as the window shows it.
    pub(crate) fn plugin_view(
        &mut self,
        p: &Plugin,
        order: usize,
        deps: &FxHashMap<String, Vec<String>>,
    ) -> PluginView {
        let table = self.session.table().clone();
        let owners: Vec<PluginOwner> = {
            let book = self.session.book();
            p.versions
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    let name = table.mod_name(v.owner).to_string();
                    let key = book.key_of(&name);
                    PluginOwner {
                        // the overwrite folder has no place in the order and is always on
                        enabled: key.is_none_or(|k| book.enabled(k)),
                        key,
                        winner: p.winner == Some(i),
                        name,
                    }
                })
                .collect()
        };
        PluginView {
            name: p.name.clone(),
            index: p.index.map(|i| i.to_string()),
            colour: p.colour,
            listed: p.listed,
            active: p.active,
            owner: p.winner.map(|w| table.mod_name(p.versions[w].owner).to_string()),
            versions: p.versions.len(),
            missing_masters: p.missing_masters.clone(),
            records: self.plugin_records(&p.name),
            states: p.states.clone(),
            caste: p.caste(),
            origin: p.origin,
            order: order as u32,
            masters: p.masters.clone(),
            gaps: p.master_gaps.clone(),
            owners,
            dependents: deps.get(&plugin_key(&p.name)).cloned().unwrap_or_default(),
        }
    }

    // @sat:app.plugins.page
    /// The whole list (`Query::Plugins`).
    pub(crate) fn plugins_page(&mut self) -> PluginsPage {
        let list = self.session.plugins();
        let deps = dependents(&list);
        let plugins = list
            .plugins
            .iter()
            .enumerate()
            .map(|(i, p)| self.plugin_view(p, i, &deps))
            .collect();
        PluginsPage {
            plugins,
            ghosts: list.ghosts.clone(),
            stats: list.stats,
        }
    }

    /// The plugins one mod has (any copy), in load order.
    pub(crate) fn mod_plugin_views(&mut self, m: ModId) -> Vec<PluginView> {
        let list = self.session.plugins();
        let deps = dependents(&list);
        list.plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| p.versions.iter().any(|v| v.owner == m))
            .map(|(i, p)| self.plugin_view(p, i, &deps))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sat_core::canon::{Entry, EntryKind, Modlist};
    use sat_core::facts::{ModFacts, PluginFacts};
    use sat_core::order::{MAIN, OrderBook};
    use sat_core::plugins::{Caste, Colour, Mark, PluginCanon, PluginState, PluginVersions};
    use sat_core::resource::Registry;
    use sat_core::session::Session;
    use sat_core::table::ProviderTable;
    use sat_formats::PluginHeader;

    use crate::{App, Command, Query, Reply};

    fn pf(name: &str, masters: &[&str]) -> PluginFacts {
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

    #[test]
    fn one_plugin_truth_in_the_list_the_mod_row_and_the_owner_toggle() {
        let facts = vec![
            ModFacts {
                name: "Lib".into(),
                plugins: vec![pf("Lib.esm", &["Skyrim.esm"])],
                ..ModFacts::default()
            },
            ModFacts {
                name: "Addon".into(),
                plugins: vec![
                    pf("Addon.esp", &["Lib.esm"]),
                    pf("Addon - Gone Patch.esp", &["Gone.esm"]),
                ],
                ..ModFacts::default()
            },
        ];
        let list = Modlist::new(
            [("Lib", false), ("Addon", true)]
                .iter()
                .map(|(n, on)| Entry {
                    name: n.to_string(),
                    kind: EntryKind::Mod,
                    enabled: *on,
                })
                .collect(),
        );
        let mut session = Session::new(
            OrderBook::from_modlist(&list),
            Arc::new(ProviderTable::build(&facts)),
        );
        session.set_plugin_facts(Arc::new(PluginVersions::build(&facts)));
        session.set_plugin_canon(PluginCanon {
            load_order: ["Skyrim.esm", "Lib.esm", "Addon.esp", "Addon - Gone Patch.esp"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            checked: vec![
                ("Lib.esm".into(), true),
                ("Addon.esp".into(), true),
                ("Addon - Gone Patch.esp".into(), false),
            ],
            blacklist: Vec::new(),
        });
        let mut a = App::new(session, Registry::with_builtins());

        let Reply::Plugins { page } = a.query(Query::Plugins).unwrap() else {
            panic!()
        };
        let addon = page.plugins.iter().find(|p| p.name == "Addon.esp").unwrap();
        assert_eq!(
            (addon.colour, addon.states.as_slice()),
            (Colour::Red, [PluginState::MasterOwnerOff].as_slice())
        );
        let lib = page.plugins.iter().find(|p| p.name == "Lib.esm").unwrap();
        assert_eq!((lib.colour, lib.caste), (Colour::Black, Caste::Esm));
        assert_eq!(lib.dependents, ["Addon.esp"]);
        assert!(!lib.owners[0].enabled && lib.owners[0].winner);
        let patch = page
            .plugins
            .iter()
            .find(|p| p.name == "Addon - Gone Patch.esp")
            .unwrap();
        assert_eq!(patch.states, [PluginState::MasterAbsent, PluginState::Unchecked]);
        assert_eq!(page.ghosts[0].name, "Gone.esm");

        // the mod row: Addon.esp loads and lacks Lib.esm (its mod is off) — red; the unchecked patch is optional
        let Reply::Rows { rows, .. } = a
            .query(Query::Rows {
                view: MAIN,
                start: 0,
                count: 10,
                badges: 0,
            })
            .unwrap()
        else {
            panic!()
        };
        let mark = rows
            .iter()
            .find(|r| r.name == "Addon")
            .unwrap()
            .plugins
            .clone()
            .unwrap();
        assert_eq!(
            (mark.top, mark.stack.as_slice()),
            (Some(Mark::Red), [Mark::Optional].as_slice())
        );

        // checking a plugin of a disabled mod turns its mod on (monolith `togglePluginEnabledByName`)
        a.command(Command::SetPluginChecked {
            name: "Lib.esm".into(),
            checked: true,
        })
        .unwrap();
        let Reply::Plugins { page } = a.query(Query::Plugins).unwrap() else {
            panic!()
        };
        let addon = page.plugins.iter().find(|p| p.name == "Addon.esp").unwrap();
        assert_eq!(
            (addon.colour, addon.states.as_slice()),
            (Colour::Green, [].as_slice())
        );
        assert!(page.plugins.iter().find(|p| p.name == "Lib.esm").unwrap().owners[0].enabled);
    }
}
