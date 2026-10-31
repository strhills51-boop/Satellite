//! Satellite domain core. @sat:core
//!
//! Layering (ADR 0002):
//! - [`engine`] — truth kinds, change sets, the projection graph (ADR 0010);
//! - [`facts`] — what scanners observed (input to the warm tier);
//! - [`canon`] — the manager's state (order, enabled, separators);
//! - [`table`] — warm tier: immutable, integer-addressed provider table built from facts;
//! - [`overwrite`] — hot tier: projection of the table under current priorities (stacks, chains, contexts);
//! - [`order`] — live load order: canon + decoration + view, mutated by row patches (ADR 0008);
//! - [`run`] — run lifecycle, staging/authoritative generations, seals before publication;
//! - [`resource`] — resource registry: manifests, trust by source, capabilities, approvals, hotkeys;
//! - [`session`] — one build: canon and facts wired to the projection graph (ADR 0010);
//! - [`sync`] — reconciling the project (canon) with the build and external donors (ADR 0006).
//!
//! This crate performs no filesystem or network I/O and compiles to WebAssembly.
//!
//! Owns: the crate layout (which module is which layer); nothing else.

pub mod canon;
pub mod engine;
pub mod facts;
pub mod ids;
pub mod interner;
pub mod order;
pub mod overwrite;
pub mod project;
pub mod resource;
pub mod run;
pub mod session;
pub mod sync;
pub mod table;

pub use ids::{ArchiveId, ChainId, ContextId, DirId, ModId, NameId, PathId};
