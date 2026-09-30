//! Tests for [`super::ref_composite`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items while keeping the source at a readable size.

use super::*;
use crate::document::{Align1, AnchorAlign, AnchorAligns};
use crate::on_demand::detect_color_mono_glyph;
use crate::pixel::PixelShape;

pub(super) fn filled_grid(w: u16, h: u16) -> PixelGrid {
    let mut g = PixelGrid::new(w, h);
    for r in 0..h {
        for c in 0..w {
            g.set(r, c, PixelShape::new(0, true));
        }
    }
    g
}

pub(super) fn make_doc(text: &str) -> Document {
    use crate::document_io::{derive_document, parse_doclines};
    let lines = parse_doclines(text);
    let (doc, _) = derive_document(&lines, std::path::PathBuf::new());
    doc
}

mod alternatives;
mod anchors;
mod caches_and_colors;
mod layout;
mod on_demand;
