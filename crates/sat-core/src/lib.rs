//! Satellite domain core.
//!
//! Layering (ADR 0002):
//! - [`facts`] — what scanners observed (input to the warm tier);
//! - [`canon`] — the manager's state (order, enabled, separators);
//! - [`table`] — warm tier: immutable, integer-addressed provider table built from facts;
//! - [`overwrite`] — hot tier: projection of the table under current priorities (stacks, chains, contexts).
//!
//! This crate performs no filesystem or network I/O and compiles to WebAssembly.

pub mod canon;
pub mod facts;
pub mod ids;
pub mod interner;
pub mod overwrite;
pub mod table;

pub use ids::{ArchiveId, ChainId, ContextId, DirId, ModId, NameId, PathId};
