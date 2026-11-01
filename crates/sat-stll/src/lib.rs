//! The Satellite project on disk (ADR 0007). @sat:stll
//!
//! - [`facts`] — the cold tier inside the project: each mod's facts (paths, sizes, metrics, archive entries,
//!   plugin headers) by entity id, with the content signature they were made from.
//!
//! Owns: the on-disk layout of a project. For now a folder (`<profile>/satellite/`: `project.v0.json` and
//! `facts/`); the single-file `.stll` packs the same sections (next step of ADR 0007).

pub mod facts;
