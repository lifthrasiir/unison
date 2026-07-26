pub mod assert;
pub mod contour;
pub mod sample;
pub mod ttf_builder;

pub use ttf_builder::{build_font_from_documents, build_font_with_gid_map, load_docs_from_directory};
#[cfg(feature = "editor")]
pub use ttf_builder::{SharedContourCache, build_font_pair_cached, new_contour_cache};

/// Problems detected while resolving `docs` into the glyph set the font build
/// will use. The build itself proceeds without them — a reference that resolves
/// to nothing simply produces no glyph — so callers that want the user to hear
/// about them have to ask.
pub fn resolution_issues(docs: &[&crate::document::Document]) -> Vec<crate::issues::Issue> {
    let name_parts = crate::document::collect_name_parts(docs);
    let expansion = ttf_builder::expand_documents(docs, &name_parts);
    crate::resolve::DocSet::new(docs).to_issues(&expansion.diagnostics)
}

pub fn ttf_to_woff2(ttf_bytes: &[u8]) -> Result<Vec<u8>, String> {
    ttf2woff2::encode(ttf_bytes, ttf2woff2::BrotliQuality::default())
        .map_err(|e| format!("WOFF2 encoding failed: {e}"))
}
