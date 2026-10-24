//! The reference semantics: a tree-walking interpreter over checked programs.
//!
//! What this crate does is what a Glyph program means, so it is part of the
//! trusted base.

#![forbid(unsafe_code)]

mod interp;

pub use interp::{Interp, Value};
