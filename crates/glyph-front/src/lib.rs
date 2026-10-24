//! Glyph's front end: source text to a loaded, checked program.
//!
//! Everything that decides what a program means lives here, including which
//! modules it is made of, so this crate is part of the trusted base.

#![forbid(unsafe_code)]

pub mod ast;
mod check;
mod error;
mod lexer;
mod loader;
mod parser;

pub use check::{check, Types};
pub use error::{json_str, Error, Pos, Sources};
pub use loader::{load, Loaded};
