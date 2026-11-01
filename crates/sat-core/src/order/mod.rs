//! Load order engine (ADR 0008): the live canon of a list of any size and its rows, changed only by operations @sat:order
//! that return row patches.
//!
//! - [`seq`] — chunked sequence with Fenwick aggregates: O(log n) position, row and block queries;
//! - [`book`] — [`OrderBook`]: canon (order, enabled), decoration (colour, logical groups), view (collapsed,
//!   filter), undo/redo, [`Patch`]es for the UI;
//! - [`cursor`] — keyboard focus, anchor and selection (keys, so they survive every mutation);
//! - [`decor`] — decoration as data for the project file, logical-group marker codec of the monolith.
//!
//! [`crate::canon::Modlist`] stays the serialized snapshot of the canon; the book is built from it and exported
//! back to it.
//!
//! Owns: the layout of the load order engine (ADR 0008).

pub mod book;
pub mod cursor;
pub mod decor;
pub mod flow;
pub mod seq;

pub use book::{
    Change, Filter, GroupId, MAIN, OrderBook, OrderError, Patch, Row, RowKind, ViewChange, ViewId,
};
pub use cursor::{Nav, Select, Shift};
pub use decor::{
    Accent, CardStyle, Decor, DecorIssue, Effect, Fill, FillKind, GroupDef, LineStyle, MarkerIssue, Outline,
    TextCase, TextStyle, Watermark, WatermarkPosition,
};
pub use seq::Key;

#[cfg(test)]
mod tests;
