pub mod assert;
pub mod contour;
pub(crate) mod glyph_cache;
pub mod sample;
pub mod ttf_builder;

pub use ttf_builder::{build_font_from_documents, build_font_with_gid_map};
#[cfg(feature = "editor")]
pub use ttf_builder::{SharedContourCache, build_font_pair_cached, new_contour_cache};

pub fn ttf_to_woff2(ttf_bytes: &[u8]) -> Result<Vec<u8>, String> {
    ttf2woff2::encode(ttf_bytes, ttf2woff2::BrotliQuality::default())
        .map_err(|e| format!("WOFF2 encoding failed: {e}"))
}
