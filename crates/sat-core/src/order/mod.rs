//! Load order engine (ADR 0008): the live canon of a list of any size and its rows, changed only by operations
//! that return row patches.
//!
//! - [`seq`] — chunked sequence with Fenwick aggregates: O(log n) position, row and block queries;
//! - [`book`] — [`OrderBook`]: canon (order, enabled), decoration (colour, logical groups), view (collapsed,
//!   filter), undo/redo, [`Patch`]es for the UI.
//!
//! [`crate::canon::Modlist`] stays the serialized snapshot of the canon; the book is built from it and exported
//! back to it.

pub mod book;
pub mod seq;

pub use book::{Change, Filter, GroupId, OrderBook, OrderError, Patch, Row, RowKind};
pub use seq::Key;

#[cfg(test)]
mod tests;
