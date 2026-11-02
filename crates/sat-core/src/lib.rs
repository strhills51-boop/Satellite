//! Satellite domain core. @sat:core
//!
//! Layering (ADR 0002):
//! - [`decisions`] — user decisions on conflicts (settled chains) by content signature;
//! - [`engine`] — truth kinds, change sets, the projection graph (ADR 0010);
//! - [`facts`] — what scanners observed (input to the warm tier);
//! - [`canon`] — the manager's state (order, enabled, separators);
//! - [`table`] — warm tier: immutable, integer-addressed provider table built from facts;
//! - [`overwrite`] — hot tier: projection of the table under current priorities (stacks, chains, contexts);
//! - [`order`] — live load order: canon + decoration + view, mutated by row patches (ADR 0008);
//! - [`run`] — run lifecycle, staging/authoritative generations, seals before publication;
//! - [`plugins`] — the load list: plugin canon, versions across mods, runtime indexes, states;
//! - [`resource`] — resource registry: manifests, trust by source, capabilities, approvals, hotkeys;
//! - [`session`] — one build: canon and facts wired to the projection graph (ADR 0010);
//! - [`sync`] — reconciling the project (canon) with the build and external donors (ADR 0006).
//!
//! This crate performs no filesystem or network I/O and compiles to WebAssembly.
//!
//! Owns: the crate layout (which module is which layer); nothing else.

pub mod canon;
pub mod decisions;
pub mod engine;
pub mod entity;
pub mod expr;
pub mod facts;
pub mod identity;
pub mod ids;
pub mod ignore;
pub mod intent;
pub mod interner;
pub mod jobs;
pub mod metrics;
pub mod order;
pub mod overwrite;
pub mod plugin_edit;
pub mod plugins;
pub mod project;
pub mod resource;
pub mod routes;
pub mod run;
pub mod session;
pub mod sync;
pub mod table;
pub mod tag_rules;
pub mod tags;

pub use ids::{ArchiveId, ChainId, ContextId, DirId, ModId, NameId, PathId};
