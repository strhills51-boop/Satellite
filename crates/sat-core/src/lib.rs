//! Satellite domain core.
//!
//! Layering (ADR 0002):
//! - [`facts`] — what scanners observed (input to the warm tier);
//! - [`canon`] — the manager's state (order, enabled, separators);
//! - [`table`] — warm tier: immutable, integer-addressed provider table built from facts;
//! - [`overwrite`] — hot tier: projection of the table under current priorities (stacks, chains, contexts);
//! - [`order`] — live load order: canon + decoration + view, mutated by row patches (ADR 0008);
//! - [`run`] — run lifecycle, staging/authoritative generations, seals before publication;
//! - [`sync`] — reconciling the project (canon) with the build and external donors (ADR 0006).
//!
//! This crate performs no filesystem or network I/O and compiles to WebAssembly.

pub mod canon;
pub mod facts;
pub mod ids;
pub mod interner;
pub mod order;
pub mod overwrite;
pub mod project;
pub mod run;
pub mod sync;
pub mod table;

pub use ids::{ArchiveId, ChainId, ContextId, DirId, ModId, NameId, PathId};
