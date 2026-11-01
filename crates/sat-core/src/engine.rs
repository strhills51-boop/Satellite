//! Engine: kinds of truth, change sets and the projection graph (ADR 0010). @sat:engine
//!
//! Owns: the vocabulary of truth kinds, `Change` (what a command changed), projection registration with
//! declared inputs, transitive invalidation and the lazy recompute plan.
//! Not here: the projections themselves (each lives with its owner module) and the views (ADR 0008/0009).
//!
//! Laws enforced here (ADR 0010 §«Законы ядра»):
//! - a projection declares its inputs; it can only depend on truth kinds and on projections registered
//!   **before** it, so the graph is acyclic by construction;
//! - invalidation is computed by the graph from a [`Change`], never listed by the command's author;
//! - nothing is recomputed eagerly: [`Graph::plan`] returns only the dirty projections a consumer needs, in
//!   dependency order.

use serde::{Deserialize, Serialize};

/// The five kinds of truth; everything else is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Truth {
    /// Order, enabled flags, separators, list membership.
    Canon,
    /// User markup over the canon that never reorders it (colour, groups, muted, collapsed).
    Decoration,
    /// Observations of the world (scans, format headers).
    Facts,
    /// User decisions on divergences and conflicts (sync record statuses, resolutions).
    Decisions,
    /// Configuration of runs, resources, policy.
    Settings,
    /// The plugin part of the canon: load order and checked flags (its own key space, plugin names).
    PluginCanon,
}

/// Which keys of a truth (or projection) changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Keys {
    All,
    /// Entity keys, sorted and unique.
    Some(Vec<u32>),
}

impl Keys {
    pub fn some(mut keys: Vec<u32>) -> Self {
        keys.sort_unstable();
        keys.dedup();
        Self::Some(keys)
    }

    /// Union (`All` absorbs).
    pub fn merge(&mut self, other: &Keys) {
        match (&mut *self, other) {
            (Keys::All, _) => {}
            (_, Keys::All) => *self = Keys::All,
            (Keys::Some(a), Keys::Some(b)) => {
                a.extend_from_slice(b);
                a.sort_unstable();
                a.dedup();
            }
        }
    }
}

// @sat:engine.change
/// What one command changed. Owners of truth return it; the engine feeds it to the graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub truth: Truth,
    pub keys: Keys,
}

pub type ProjectionId = usize;

/// An input of a projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Input {
    Truth(Truth),
    /// Reads another projection's output as a whole: any change upstream dirties this one with `All`.
    Projection(ProjectionId),
    /// Reads a projection whose output is keyed by the same entity keys as its own inputs (e.g. priorities:
    /// one value per canon entry). Upstream keys pass through, so a downstream projection can recompute only
    /// what those keys touch.
    Keyed(ProjectionId),
}

impl Input {
    fn upstream(self) -> Option<ProjectionId> {
        match self {
            Input::Projection(p) | Input::Keyed(p) => Some(p),
            Input::Truth(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GraphError {
    #[error("projection {0:?} registered twice")]
    Duplicate(String),
    #[error("projection {name:?} depends on unknown projection #{input}")]
    UnknownInput { name: String, input: ProjectionId },
    #[error("projection {0:?} has no inputs")]
    NoInputs(String),
}

#[derive(Debug, Clone)]
struct Node {
    name: String,
    inputs: Vec<Input>,
    /// Projections that read this one.
    dependents: Vec<ProjectionId>,
    /// `Some(keys)` = dirty with these changed input keys.
    dirty: Option<Keys>,
}

// @sat:engine.graph
/// The projection graph. Registration order is a topological order.
#[derive(Debug, Clone, Default)]
pub struct Graph {
    nodes: Vec<Node>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a projection. Inputs may only be truth kinds and already registered projections. A new
    /// projection starts dirty (it has never been computed).
    pub fn register(&mut self, name: &str, inputs: &[Input]) -> Result<ProjectionId, GraphError> {
        if self.nodes.iter().any(|n| n.name == name) {
            return Err(GraphError::Duplicate(name.to_string()));
        }
        if inputs.is_empty() {
            return Err(GraphError::NoInputs(name.to_string()));
        }
        let id = self.nodes.len();
        for i in inputs {
            if let Some(p) = i.upstream()
                && p >= id
            {
                return Err(GraphError::UnknownInput {
                    name: name.to_string(),
                    input: p,
                });
            }
        }
        for i in inputs {
            if let Some(p) = i.upstream()
                && !self.nodes[p].dependents.contains(&id)
            {
                self.nodes[p].dependents.push(id);
            }
        }
        self.nodes.push(Node {
            name: name.to_string(),
            inputs: inputs.to_vec(),
            dependents: Vec::new(),
            dirty: Some(Keys::All),
        });
        Ok(id)
    }

    pub fn id(&self, name: &str) -> Option<ProjectionId> {
        self.nodes.iter().position(|n| n.name == name)
    }

    pub fn name(&self, id: ProjectionId) -> &str {
        &self.nodes[id].name
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn is_dirty(&self, id: ProjectionId) -> bool {
        self.nodes[id].dirty.is_some()
    }

    /// Changed input keys accumulated since the projection was last computed (for incremental recompute).
    pub fn pending(&self, id: ProjectionId) -> Option<&Keys> {
        self.nodes[id].dirty.as_ref()
    }

    /// Marks every projection that depends on the changed truth dirty, transitively. Direct readers of the
    /// truth get the changed keys; readers through [`Input::Keyed`] get the keys their upstream got in this
    /// change; readers through [`Input::Projection`] get `All`. Returns the projections that became dirty or
    /// got more keys.
    pub fn apply(&mut self, change: &Change) -> Vec<ProjectionId> {
        // keys each projection received from this change (registration order is topological)
        let mut got: Vec<Option<Keys>> = vec![None; self.nodes.len()];
        for id in 0..self.nodes.len() {
            let mut keys: Option<Keys> = None;
            let mut add = |k: &Keys| match &mut keys {
                Some(acc) => acc.merge(k),
                None => keys = Some(k.clone()),
            };
            for &i in &self.nodes[id].inputs {
                match i {
                    Input::Truth(t) if t == change.truth => add(&change.keys),
                    Input::Truth(_) => {}
                    Input::Keyed(p) => {
                        if let Some(k) = &got[p] {
                            add(k);
                        }
                    }
                    Input::Projection(p) => {
                        if got[p].is_some() {
                            add(&Keys::All);
                        }
                    }
                }
            }
            if let Some(k) = keys {
                let node = &mut self.nodes[id];
                match &mut node.dirty {
                    Some(d) => d.merge(&k),
                    None => node.dirty = Some(k.clone()),
                }
                got[id] = Some(k);
            }
        }
        (0..self.nodes.len()).filter(|&i| got[i].is_some()).collect()
    }

    /// The dirty projections needed to bring `wanted` up to date, in dependency order: every dirty
    /// projection among `wanted` and their transitive inputs. Clean ones and ones nobody wants are skipped
    /// (laziness).
    pub fn plan(&self, wanted: &[ProjectionId]) -> Vec<ProjectionId> {
        let mut need = vec![false; self.nodes.len()];
        let mut stack: Vec<ProjectionId> = wanted.to_vec();
        while let Some(id) = stack.pop() {
            if need[id] {
                continue;
            }
            need[id] = true;
            for i in &self.nodes[id].inputs {
                if let Some(p) = i.upstream() {
                    stack.push(p);
                }
            }
        }
        (0..self.nodes.len())
            .filter(|&id| need[id] && self.nodes[id].dirty.is_some())
            .collect()
    }

    /// Records that a projection was recomputed. Its inputs must be clean (computed before it).
    pub fn done(&mut self, id: ProjectionId) {
        debug_assert!(
            self.nodes[id]
                .inputs
                .iter()
                .all(|i| i.upstream().is_none_or(|p| self.nodes[p].dirty.is_none())),
            "{} computed before its inputs",
            self.nodes[id].name
        );
        self.nodes[id].dirty = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> (Graph, [ProjectionId; 4]) {
        let mut g = Graph::new();
        let table = g.register("table", &[Input::Truth(Truth::Facts)]).unwrap();
        let prio = g.register("priorities", &[Input::Truth(Truth::Canon)]).unwrap();
        let overwrite = g
            .register("overwrite", &[Input::Projection(table), Input::Projection(prio)])
            .unwrap();
        let rows = g
            .register(
                "rows",
                &[
                    Input::Truth(Truth::Canon),
                    Input::Truth(Truth::Decoration),
                    Input::Projection(overwrite),
                ],
            )
            .unwrap();
        for id in [table, prio, overwrite, rows] {
            g.done(id);
        }
        (g, [table, prio, overwrite, rows])
    }

    #[test]
    fn keyed_inputs_pass_keys_through() {
        let mut g = Graph::new();
        let conflicts = g.register("conflicts", &[Input::Truth(Truth::Facts)]).unwrap();
        let prio = g
            .register(
                "priorities",
                &[Input::Truth(Truth::Canon), Input::Truth(Truth::Facts)],
            )
            .unwrap();
        let ow = g
            .register("overwrite", &[Input::Projection(conflicts), Input::Keyed(prio)])
            .unwrap();
        for id in [conflicts, prio, ow] {
            g.done(id);
        }
        g.apply(&Change {
            truth: Truth::Canon,
            keys: Keys::some(vec![7, 3]),
        });
        g.apply(&Change {
            truth: Truth::Canon,
            keys: Keys::some(vec![9]),
        });
        assert_eq!(
            g.pending(ow),
            Some(&Keys::Some(vec![3, 7, 9])),
            "keys accumulate through Keyed"
        );
        assert!(!g.is_dirty(conflicts));
        g.apply(&Change {
            truth: Truth::Facts,
            keys: Keys::All,
        });
        assert_eq!(
            g.pending(ow),
            Some(&Keys::All),
            "a whole-input change absorbs keys"
        );
        assert_eq!(g.plan(&[ow]), vec![conflicts, prio, ow]);
    }

    #[test]
    fn registration_keeps_the_graph_acyclic() {
        let mut g = Graph::new();
        assert!(matches!(
            g.register("x", &[Input::Projection(0)]),
            Err(GraphError::UnknownInput { .. })
        ));
        g.register("a", &[Input::Truth(Truth::Canon)]).unwrap();
        assert!(matches!(
            g.register("a", &[Input::Truth(Truth::Canon)]),
            Err(GraphError::Duplicate(_))
        ));
        assert!(matches!(g.register("b", &[]), Err(GraphError::NoInputs(_))));
        assert!(g.is_dirty(0), "never computed");
    }

    #[test]
    fn a_move_dirties_what_depends_on_the_canon_only() {
        let (mut g, [table, prio, overwrite, rows]) = graph();
        let touched = g.apply(&Change {
            truth: Truth::Canon,
            keys: Keys::some(vec![7, 3, 7]),
        });
        assert_eq!(touched, vec![prio, overwrite, rows]);
        assert!(!g.is_dirty(table), "facts did not change");
        assert_eq!(g.pending(prio), Some(&Keys::Some(vec![3, 7])));
        assert_eq!(g.pending(overwrite), Some(&Keys::All));
        // a colour change dirties only the rows
        let (mut g, [_, prio, overwrite, rows]) = graph();
        assert_eq!(
            g.apply(&Change {
                truth: Truth::Decoration,
                keys: Keys::some(vec![1]),
            }),
            vec![rows]
        );
        assert!(!g.is_dirty(prio) && !g.is_dirty(overwrite));
    }

    #[test]
    fn plan_is_lazy_and_ordered() {
        let (mut g, [table, prio, overwrite, rows]) = graph();
        g.apply(&Change {
            truth: Truth::Facts,
            keys: Keys::All,
        });
        g.apply(&Change {
            truth: Truth::Canon,
            keys: Keys::some(vec![1]),
        });
        // someone only needs the priorities: nothing else is computed
        assert_eq!(g.plan(&[prio]), vec![prio]);
        // the rows need everything upstream, in dependency order
        assert_eq!(g.plan(&[rows]), vec![table, prio, overwrite, rows]);
        for id in g.plan(&[rows]) {
            g.done(id);
        }
        assert!(g.plan(&[rows]).is_empty());
    }

    #[test]
    fn keys_merge() {
        let mut k = Keys::some(vec![5, 1]);
        k.merge(&Keys::some(vec![1, 9]));
        assert_eq!(k, Keys::Some(vec![1, 5, 9]));
        k.merge(&Keys::All);
        assert_eq!(k, Keys::All);
        k.merge(&Keys::some(vec![2]));
        assert_eq!(k, Keys::All);
    }
}
