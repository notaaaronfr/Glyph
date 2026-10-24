//! The native backend: checked programs to C, plus the C runtime they link.
//!
//! Trusted only for claims about native binaries; the interpreter in
//! `glyph-run` defines what a program means.

#![forbid(unsafe_code)]

mod codegen;

pub use codegen::{emit_c, Target};
