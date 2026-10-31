//! Synchronization between the project (canon) and the sides it is reconciled with (ADR 0006).
//!
//! - [`names`] — name identity rules shared by reconcile and donor matching (monolith parity).
//! - [`donor`] — external donor sessions: another build's `modlist.txt` + mods folder, matched against the project.
//! - [`records`] — sync records and the pure disk ↔ project reconcile.

pub mod donor;
pub mod names;
pub mod records;
