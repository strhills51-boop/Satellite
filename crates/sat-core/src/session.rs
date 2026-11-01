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

use crate::engine::{Change, Graph, Input, Keys, ProjectionId, Truth};
use crate::ids::ModId;
use crate::order::{self, OrderBook, OrderError};
use crate::overwrite::{ConflictIndex, OverwriteProjection, Priorities};
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
    p_overwrite: ProjectionId,
    conflicts: Option<Arc<ConflictIndex>>,
    priorities: Option<Arc<Priorities>>,
    overwrite: Option<Arc<OverwriteProjection>>,
    /// How the last overwrite was brought up to date (observability, tests).
    last_overwrite: Recompute,
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
            .register("conflicts", &[Input::Truth(Truth::Facts)])
            .expect("fresh graph");
        let p_priorities = graph
            .register(
                "priorities",
                &[Input::Truth(Truth::Canon), Input::Truth(Truth::Facts)],
            )
            .expect("fresh graph");
        let p_overwrite = graph
            .register(
                "overwrite",
                &[Input::Projection(p_conflicts), Input::Keyed(p_priorities)],
            )
            .expect("fresh graph");
        Self {
            index: index_of(&table),
            book,
            table,
            graph,
            p_conflicts,
            p_priorities,
            p_overwrite,
            conflicts: None,
            priorities: None,
            overwrite: None,
            last_overwrite: Recompute::Full,
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

    /// New facts (a finished run): replaces the table and dirties everything that reads facts.
    pub fn set_facts(&mut self, table: Arc<ProviderTable>) {
        self.index = index_of(&table);
        self.table = table;
        self.graph.apply(&Change {
            truth: Truth::Facts,
            keys: Keys::All,
        });
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
            if p == self.p_conflicts {
                self.conflicts = Some(Arc::new(ConflictIndex::build(&self.table)));
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
                        Arc::make_mut(&mut prev).update_in_place(&self.table, prio, &touched);
                        self.overwrite = Some(prev);
                    }
                    _ => {
                        self.last_overwrite = Recompute::Full;
                        self.overwrite = Some(Arc::new(OverwriteProjection::compute_with(
                            index,
                            &self.table,
                            prio,
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
