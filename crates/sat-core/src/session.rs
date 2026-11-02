//! Session: one build's truth owners wired to the engine's projection graph (ADR 0010). @sat:session
//!
//! Owns: the wiring of the live canon (`OrderBook`) and the facts (`ProviderTable`) to the derived priorities
//! and overwrite projection: commands go through the session, their change sets dirty the graph, projections
//! are recomputed lazily when someone asks for them.
//! Not here: the projections' algorithms (`@sat:canon.priorities`, `@sat:overwrite.projection`), the graph
//! (`@sat:engine.graph`), the book's operations (`@sat:order.book`).
//!
//! What this buys: a colour or a filter change does not recompute the overwrite projection; a move recomputes
//! it once, on the next read, however many moves happened in between.

use std::sync::Arc;

use rustc_hash::FxHashMap;

use crate::decisions::ChainDecisions;
use crate::engine::{Change, Graph, Input, Keys, ProjectionId, Truth};
use crate::ids::ModId;
use crate::order::{self, OrderBook, OrderError};
use crate::overwrite::{ConflictIndex, OverwriteProjection, Priorities};
use crate::plugins::{PluginCanon, PluginList, PluginVersions};
use crate::table::ProviderTable;

/// Name of the manager's output folder treated as the highest priority (MO2 `overwrite`).
pub const OVERWRITE: &str = "Overwrite";

/// How a projection was brought up to date.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recompute {
    Full,
    /// Only stacks of paths provided by `touched` mods were recomputed.
    Incremental {
        touched: usize,
    },
}

// @sat:session.state
/// A build being worked on: canon, facts, and the projections derived from them.
#[derive(Debug)]
pub struct Session {
    book: OrderBook,
    table: Arc<ProviderTable>,
    /// Folded mod name → table id.
    index: FxHashMap<String, ModId>,
    graph: Graph,
    p_conflicts: ProjectionId,
    p_priorities: ProjectionId,
    p_units: ProjectionId,
    p_overwrite: ProjectionId,
    p_plugins: ProjectionId,
    plugin_versions: Arc<PluginVersions>,
    plugin_canon: PluginCanon,
    /// Paths that are no conflict (`@sat:ignore`).
    ignore: crate::ignore::IgnoreRules,
    plugins: Option<Arc<PluginList>>,
    conflicts: Option<Arc<ConflictIndex>>,
    priorities: Option<Arc<Priorities>>,
    overwrite: Option<Arc<OverwriteProjection>>,
    /// How the last overwrite was brought up to date (observability, tests).
    last_overwrite: Recompute,
    /// User decisions on conflicts (settled chains).
    decisions: ChainDecisions,
    /// Logical groups computed as one (`@sat:overwrite.units`), from the book.
    units: crate::overwrite::Units,
    /// The group name of each group's representative.
    unit_names: FxHashMap<ModId, String>,
    /// Stable key by folded mod name (the entity id): survives a new table.
    key_map: FxHashMap<String, String>,
    /// The same by `ModId` index of the current table; `None` → the name is the key.
    keys: Vec<Option<String>>,
    /// Bumped when the keys change: caches of chain signatures read it.
    keys_rev: u64,
    /// How many times each projection was computed (observability, tests).
    computed: FxHashMap<ProjectionId, u64>,
}

fn index_of(table: &ProviderTable) -> FxHashMap<String, ModId> {
    table
        .mod_names()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.to_lowercase(), ModId::from_idx(i)))
        .collect()
}

impl Session {
    pub fn new(book: OrderBook, table: Arc<ProviderTable>) -> Self {
        let mut graph = Graph::new();
        let p_conflicts = graph
            .register(
                "conflicts",
                &[Input::Truth(Truth::Facts), Input::Truth(Truth::Settings)],
            )
            .expect("fresh graph");
        let p_priorities = graph
            .register(
                "priorities",
                &[Input::Truth(Truth::Canon), Input::Truth(Truth::Facts)],
            )
            .expect("fresh graph");
        // logical groups computed as one (`@sat:overwrite.units`): decoration, and the canon (a move can break a
        // group's contiguity); keyed, so the overwrite recomputes only what the changed entries touch
        let p_units = graph
            .register(
                "units",
                &[
                    Input::Truth(Truth::Canon),
                    Input::Truth(Truth::Facts),
                    Input::Truth(Truth::Groups),
                ],
            )
            .expect("fresh graph");
        let p_overwrite = graph
            .register(
                "overwrite",
                &[
                    Input::Projection(p_conflicts),
                    Input::Keyed(p_priorities),
                    Input::Keyed(p_units),
                ],
            )
            .expect("fresh graph");
        let p_plugins = graph
            .register(
                "plugins",
                &[
                    Input::Truth(Truth::Facts),
                    Input::Truth(Truth::PluginCanon),
                    Input::Projection(p_priorities),
                ],
            )
            .expect("fresh graph");
        Self {
            index: index_of(&table),
            book,
            table,
            graph,
            p_conflicts,
            p_priorities,
            p_units,
            p_overwrite,
            p_plugins,
            plugin_versions: Arc::new(PluginVersions::default()),
            plugin_canon: PluginCanon::default(),
            ignore: Default::default(),
            plugins: None,
            conflicts: None,
            priorities: None,
            overwrite: None,
            last_overwrite: Recompute::Full,
            decisions: ChainDecisions::default(),
            units: Default::default(),
            unit_names: Default::default(),
            key_map: FxHashMap::default(),
            keys: Vec::new(),
            keys_rev: 0,
            computed: FxHashMap::default(),
        }
    }

    pub fn book(&self) -> &OrderBook {
        &self.book
    }

    pub fn table(&self) -> &Arc<ProviderTable> {
        &self.table
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    // @sat:session.command
    /// Runs a command on the book; its truth changes dirty the graph. Every change to the canon goes through here.
    /// Accepts operations returning `Change` or `Option<Change>`.
    pub fn command<C: Into<Option<order::Change>>>(
        &mut self,
        f: impl FnOnce(&mut OrderBook) -> Result<C, OrderError>,
    ) -> Result<Option<order::Change>, OrderError> {
        let change = f(&mut self.book)?.into();
        if let Some(c) = &change {
            for t in &c.truth {
                self.graph.apply(t);
            }
        }
        Ok(change)
    }

    /// Runs a command resource by id in view `v` (key press, menu, palette): the registry checks it is active
    /// and holds the capabilities its operation needs; the change goes through [`Session::command`].
    pub fn run(
        &mut self,
        registry: &crate::resource::Registry,
        id: &str,
        v: order::ViewId,
    ) -> Result<Option<order::Change>, crate::resource::CommandError> {
        let action = crate::resource::command::authorize(registry.get(id), id)?;
        Ok(self.command(|bk| action.apply(bk, v))?)
    }

    pub fn decisions(&self) -> &ChainDecisions {
        &self.decisions
    }

    /// Replaces the decisions (opening a project); badges and the summary re-read.
    pub fn set_decisions(&mut self, decisions: ChainDecisions) {
        self.decisions = decisions;
        self.graph.apply(&Change {
            truth: Truth::Decisions,
            keys: Keys::All,
        });
    }

    // @sat:session.member-keys
    /// Gives every table mod its stable key (the project's entity id, `@sat:entity`): chain signatures — and so
    /// chain decisions, icons and hues — hash keys, not folder names, so renaming a mod keeps its chains settled.
    /// A mod without a key (no project yet, the overwrite folder) is keyed by its name.
    pub fn set_member_keys(&mut self, keys: impl IntoIterator<Item = (String, String)>) {
        let map: FxHashMap<String, String> = keys.into_iter().map(|(n, k)| (n.to_lowercase(), k)).collect();
        if map == self.key_map {
            return;
        }
        self.key_map = map;
        self.rekey_table();
    }

    fn rekey_table(&mut self) {
        let keys: Vec<Option<String>> = self
            .table
            .mod_names()
            .iter()
            .map(|n| self.key_map.get(&n.to_lowercase()).cloned())
            .collect();
        if keys == self.keys {
            return;
        }
        self.keys = keys;
        self.keys_rev += 1;
        self.graph.apply(&Change {
            truth: Truth::Decisions,
            keys: Keys::All,
        });
    }

    /// The stable key of table mod `m`: its entity id, else its name.
    pub fn member_key(&self, m: ModId) -> &str {
        match self.keys.get(m.idx()) {
            Some(Some(k)) => k,
            _ => self.table.mod_name(m),
        }
    }

    /// The entity id of table mod `m`, if the project knows it.
    pub fn member_id(&self, m: ModId) -> Option<&str> {
        self.keys.get(m.idx())?.as_deref()
    }

    /// What a stack member is called for people: the group's name for a group computed as one, else the mod's.
    pub fn unit_label(&self, m: ModId) -> &str {
        self.unit_names
            .get(&m)
            .map_or_else(|| self.table.mod_name(m), String::as_str)
    }

    /// The table id of a mod by name (any case).
    pub fn mod_id(&self, name: &str) -> Option<ModId> {
        self.index.get(&name.to_lowercase()).copied()
    }

    /// Revision of the member keys (cache key for anything hashed from them).
    pub fn member_keys_rev(&self) -> u64 {
        self.keys_rev
    }

    // @sat:session.decide
    /// Marks a chain (by content signature) settled or not; the change goes to the graph as `Truth::Decisions`.
    /// Returns whether anything changed.
    pub fn set_chain_done(&mut self, signature: u64, done: bool) -> bool {
        match self.decisions.set(signature, done) {
            Some(c) => {
                self.graph.apply(&c);
                true
            }
            None => false,
        }
    }

    /// Plugin facts of the same run as the table (versions across mods).
    pub fn set_plugin_facts(&mut self, versions: Arc<PluginVersions>) {
        self.plugin_versions = versions;
        self.graph.apply(&Change {
            truth: Truth::Facts,
            keys: Keys::All,
        });
    }

    /// The manager's plugin lists (import, or after an edit of the plugin order).
    pub fn conflict_ignore(&self) -> &crate::ignore::IgnoreRules {
        &self.ignore
    }

    // @sat:session.ignore
    /// Replaces the conflict ignore rules: the conflict index is rebuilt (a Settings change), the overwrite follows.
    pub fn set_conflict_ignore(&mut self, rules: crate::ignore::IgnoreRules) {
        if rules == self.ignore {
            return;
        }
        self.ignore = rules;
        self.graph.apply(&Change {
            truth: Truth::Settings,
            keys: Keys::All,
        });
    }

    pub fn plugin_canon(&self) -> &PluginCanon {
        &self.plugin_canon
    }

    pub fn set_plugin_canon(&mut self, canon: PluginCanon) {
        self.plugin_canon = canon;
        self.graph.apply(&Change {
            truth: Truth::PluginCanon,
            keys: Keys::All,
        });
    }

    // @sat:session.plugins
    /// Current plugin list (recomputed when facts, the plugin canon or mod priorities change; a few thousand
    /// plugins, so a full recompute — ADR 0010 law 11 targets inputs of millions).
    pub fn plugins(&mut self) -> Arc<PluginList> {
        self.bring_up_to_date(self.p_plugins);
        self.plugins.clone().expect("computed")
    }

    /// New facts (a finished run): replaces the table and dirties everything that reads facts.
    pub fn set_facts(&mut self, table: Arc<ProviderTable>) {
        self.index = index_of(&table);
        self.table = table;
        self.graph.apply(&Change {
            truth: Truth::Facts,
            keys: Keys::All,
        });
        self.rekey_table();
    }

    /// Current priorities (recomputed only when the canon or facts changed).
    pub fn priorities(&mut self) -> Arc<Priorities> {
        self.bring_up_to_date(self.p_priorities);
        self.priorities.clone().expect("computed")
    }

    // @sat:session.overwrite
    /// Current overwrite projection (recomputed only when the canon or facts changed).
    pub fn overwrite(&mut self) -> Arc<OverwriteProjection> {
        self.bring_up_to_date(self.p_overwrite);
        self.overwrite.clone().expect("computed")
    }

    /// How many times a projection was computed since the session started.
    pub fn computed(&self, name: &str) -> u64 {
        self.graph
            .id(name)
            .and_then(|id| self.computed.get(&id).copied())
            .unwrap_or(0)
    }

    /// How the last overwrite recomputation went.
    pub fn last_overwrite(&self) -> Recompute {
        self.last_overwrite
    }

    /// Mods whose stacks may have changed since `old`: the mods of the changed canon keys, plus every mod
    /// whose active state flipped (renames, membership). Other mods keep their relative order.
    fn touched_mods(&self, keys: &[u32], old: &Priorities, new: &Priorities) -> Vec<ModId> {
        let mut out: Vec<ModId> = keys
            .iter()
            .filter_map(|&k| self.index.get(&self.book.name(k).to_lowercase()).copied())
            .collect();
        out.extend(
            (0..new.0.len())
                .map(ModId::from_idx)
                .filter(|&m| old.is_active(m) != new.is_active(m)),
        );
        out.sort_unstable();
        out.dedup();
        out
    }

    fn bring_up_to_date(&mut self, id: ProjectionId) {
        for p in self.graph.plan(&[id]) {
            if p == self.p_plugins {
                let prio = self.priorities.as_ref().expect("inputs first");
                self.plugins = Some(Arc::new(PluginList::compute(
                    &self.plugin_versions,
                    &self.plugin_canon,
                    prio,
                )));
            } else if p == self.p_conflicts {
                self.conflicts = Some(Arc::new(ConflictIndex::build_with(&self.table, &self.ignore)));
            } else if p == self.p_priorities {
                let top = self.index.get(&OVERWRITE.to_lowercase()).copied();
                let index = &self.index;
                let prio = crate::canon::priorities(
                    self.book.entries(),
                    self.table.mod_count(),
                    |n| index.get(&n.to_lowercase()).copied(),
                    top,
                );
                self.priorities = Some(Arc::new(prio));
            } else if p == self.p_units {
                let index = &self.index;
                let mut names = FxHashMap::default();
                let groups: Vec<Vec<ModId>> = self
                    .book
                    .group_members()
                    .into_iter()
                    .map(|(name, g)| {
                        let ms: Vec<ModId> = g
                            .into_iter()
                            .filter_map(|k| index.get(&self.book.name(k).to_lowercase()).copied())
                            .collect();
                        if ms.len() > 1 {
                            names.insert(ms[0], name);
                        }
                        ms
                    })
                    .collect();
                self.units = crate::overwrite::Units::of_groups(self.table.mod_count(), &groups);
                self.unit_names = names;
            } else if p == self.p_overwrite {
                let prio = self.priorities.as_ref().expect("inputs first").as_ref().clone();
                let index = self.conflicts.clone().expect("inputs first");
                let keys = self.graph.pending(p).cloned();
                match (self.overwrite.take(), keys) {
                    (Some(mut prev), Some(Keys::Some(keys))) if Arc::ptr_eq(prev.index(), &index) => {
                        let touched = self.touched_mods(&keys, prev.priorities(), &prio);
                        self.last_overwrite = Recompute::Incremental {
                            touched: touched.len(),
                        };
                        // in place when no reader holds the previous snapshot, otherwise on a copy
                        Arc::make_mut(&mut prev).update_units_in_place(
                            &self.table,
                            prio,
                            self.units.clone(),
                            &touched,
                        );
                        self.overwrite = Some(prev);
                    }
                    _ => {
                        self.last_overwrite = Recompute::Full;
                        self.overwrite = Some(Arc::new(OverwriteProjection::compute_units(
                            index,
                            &self.table,
                            prio,
                            self.units.clone(),
                        )));
                    }
                }
            }
            *self.computed.entry(p).or_default() += 1;
            self.graph.done(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canon::{Entry, EntryKind, Modlist};
    use crate::order::MAIN;
    use crate::table::tests::mk;

    fn session() -> Session {
        let table = ProviderTable::build(&[
            mk("A", &["textures/a.dds", "meshes/x.nif"]),
            mk("B", &["textures/a.dds"]),
            mk("C", &["meshes/x.nif"]),
        ]);
        let list = Modlist::new(
            ["S_separator", "A", "B", "C"]
                .iter()
                .map(|n| Entry {
                    name: n.to_string(),
                    kind: if n.ends_with("_separator") {
                        EntryKind::Separator
                    } else {
                        EntryKind::Mod
                    },
                    enabled: true,
                })
                .collect(),
        );
        Session::new(OrderBook::from_modlist(&list), Arc::new(table))
    }

    #[test]
    fn a_logical_group_is_computed_as_one_mod() {
        let mut s = session();
        let (a, b) = (s.book().key_of("A").unwrap(), s.book().key_of("B").unwrap());
        let ma = s.mod_id("A").unwrap();
        let mb = s.mod_id("B").unwrap();
        assert_eq!(
            s.overwrite().totals().conflict_paths,
            2,
            "A×B on the texture, A×C on the mesh"
        );
        s.command(|bk| bk.create_group("AB", &[a, b]).map(|x| x.1))
            .unwrap();
        let ow = s.overwrite();
        // A and B share the texture inside their group: no conflict there; the mesh is the group against C
        assert_eq!(ow.totals().conflict_paths, 1);
        assert_eq!(ow.units().rep(mb), ma);
        let mesh = s.table().find_path("meshes/x.nif").unwrap();
        let chain = ow.path_chain(mesh).unwrap();
        assert_eq!(ow.chain_mods(chain), [s.mod_id("C").unwrap(), ma]);
        assert!(!ow.stat(mb).fully_overridden, "a member is judged with its group");
        // dissolving the group brings the inner conflict back (incrementally)
        let g = s.book().group_ids()[0];
        s.command(|bk| bk.dissolve_group(g)).unwrap();
        assert_eq!(s.overwrite().totals().conflict_paths, 2);
        assert!(matches!(s.last_overwrite(), Recompute::Incremental { .. }));
    }

    #[test]
    fn projections_follow_the_truth_lazily() {
        let mut s = session();
        let a = s.book().key_of("A").unwrap();
        let b = s.book().key_of("B").unwrap();
        let ow = s.overwrite();
        let path = s.table().find_path("textures/a.dds").unwrap();
        assert_eq!(s.table().mod_name(ow.path_winner(path).unwrap()), "B");
        assert_eq!(s.computed("overwrite"), 1);

        // decoration and view operations do not touch the projection
        s.command(|bk| bk.set_color(&[a], Some(1))).unwrap();
        s.command(|bk| bk.set_collapsed(MAIN, bk.key_of("S_separator").unwrap(), true))
            .unwrap();
        s.overwrite();
        assert_eq!(s.computed("overwrite"), 1, "colour / fold must not recompute");

        // several moves: one recomputation on the next read
        s.command(|bk| bk.move_keys(&[a], None)).unwrap();
        s.command(|bk| bk.move_keys(&[b], Some(a))).unwrap();
        s.command(|bk| bk.move_keys(&[a], Some(b))).unwrap();
        assert_eq!(s.computed("overwrite"), 1, "nothing computed until read");
        let ow = s.overwrite();
        assert_eq!(s.computed("overwrite"), 2);
        // order is now S, A, B, C → B still wins a.dds; disable B → A wins
        assert_eq!(s.table().mod_name(ow.path_winner(path).unwrap()), "B");
        s.command(|bk| bk.set_enabled(&[b], Some(false))).unwrap();
        let ow = s.overwrite();
        assert_eq!(s.table().mod_name(ow.path_winner(path).unwrap()), "A");
        assert_eq!(s.computed("overwrite"), 3);
        assert_eq!(s.computed("priorities"), 3);
        assert_eq!(s.last_overwrite(), Recompute::Incremental { touched: 1 });
        assert_eq!(
            s.computed("conflicts"),
            1,
            "the conflict structure is built once per facts"
        );
        let prio = s.priorities().as_ref().clone();
        let full = OverwriteProjection::compute(s.table(), prio);
        assert_eq!(full.stats(), ow.stats());
        assert_eq!(full.totals(), ow.totals());
    }

    #[test]
    fn ignore_rules_rebuild_conflicts_and_the_overwrite_follows() {
        let mut s = session();
        assert_eq!(s.overwrite().totals().conflict_paths, 2);
        s.set_conflict_ignore(crate::ignore::IgnoreRules::new(["meshes/"]));
        assert_eq!(s.overwrite().totals().conflict_paths, 1);
        assert_eq!(s.computed("conflicts"), 2);
        // the same rules again change nothing
        s.set_conflict_ignore(crate::ignore::IgnoreRules::new(["Meshes\\"]));
        s.overwrite();
        assert_eq!(s.computed("conflicts"), 2);
    }

    #[test]
    fn commands_by_id_reach_the_graph() {
        let mut s = session();
        let r = crate::resource::Registry::with_builtins();
        s.overwrite();
        s.run(&r, "core.cursor.down", MAIN).unwrap(); // focus the separator
        s.run(&r, "core.cursor.down", MAIN).unwrap(); // focus A
        s.overwrite();
        assert_eq!(s.computed("overwrite"), 1, "cursor moves are view-only");
        s.run(&r, "core.order.toggle-enabled", MAIN).unwrap();
        s.overwrite();
        assert_eq!(s.computed("overwrite"), 2);
        assert!(s.run(&r, "core.nope", MAIN).is_err());
    }

    #[test]
    fn plugins_follow_mods_and_ignore_decoration() {
        use crate::facts::{ModFacts, PluginFacts};
        let mut s = session();
        let pf = |n: &str, masters: &[&str]| PluginFacts {
            file_name: n.into(),
            header: Some(sat_formats::PluginHeader {
                masters: masters.iter().map(|m| m.to_string()).collect(),
                ..Default::default()
            }),
            records: None,
            error: None,
        };
        let mods = [
            ModFacts {
                name: "A".into(),
                plugins: vec![pf("P.esp", &["Skyrim.esm"])],
                ..Default::default()
            },
            ModFacts {
                name: "B".into(),
                plugins: vec![pf("P.esp", &["Skyrim.esm", "Update.esm"])],
                ..Default::default()
            },
            ModFacts {
                name: "C".into(),
                ..Default::default()
            },
        ];
        s.set_plugin_facts(Arc::new(PluginVersions::build(&mods)));
        s.set_plugin_canon(PluginCanon {
            load_order: vec!["Skyrim.esm".into(), "P.esp".into()],
            checked: vec![("P.esp".into(), true)],
            ..PluginCanon::default()
        });
        let masters = |s: &mut Session| s.plugins().get("P.esp").unwrap().masters.len();
        assert_eq!(masters(&mut s), 2, "B is below A in the list = higher priority");
        let n = s.computed("plugins");
        let a = s.book().key_of("A").unwrap();
        s.command(|bk| bk.set_color(&[a], Some(3))).unwrap();
        s.plugins();
        assert_eq!(s.computed("plugins"), n, "decoration does not touch plugins");
        let b = s.book().key_of("B").unwrap();
        s.command(|bk| bk.set_enabled(&[b], Some(false))).unwrap();
        assert_eq!(masters(&mut s), 1, "B off: A's version is served");
    }

    #[test]
    fn new_facts_recompute() {
        let mut s = session();
        s.overwrite();
        let t = ProviderTable::build(&[mk("A", &["x.txt"]), mk("B", &["x.txt"]), mk("C", &["y.txt"])]);
        s.set_facts(Arc::new(t));
        let ow = s.overwrite();
        assert_eq!(s.computed("overwrite"), 2);
        assert_eq!(s.computed("conflicts"), 2);
        assert_eq!(s.last_overwrite(), Recompute::Full);
        assert_eq!(ow.totals().conflict_paths, 1);
    }
}
