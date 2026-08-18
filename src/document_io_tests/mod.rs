//! Tests for [`super::document_io`]: parser round-trips and derivation.
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items while keeping the source at a readable size.

mod asserts;
mod at_names;
mod colors;
mod comments;
mod derive;
mod doclines;
mod lenient;
mod maps;
mod misc;
mod roundtrip;
mod tokenizer;

use super::*;
