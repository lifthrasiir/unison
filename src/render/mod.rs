pub mod assert;
pub mod contour;
pub(crate) mod glyph_cache;
pub mod sample;
pub mod ttf_builder;

#[cfg(feature = "editor")]
pub use ttf_builder::{
    BuiltFontPair, SharedContourCache, build_font_pair_cached, build_font_pair_cached_for,
    build_font_with_gid_map_for_cached, new_contour_cache,
};
#[cfg(any(feature = "editor", test))]
pub use ttf_builder::build_font_from_documents;
pub use ttf_builder::{FontWithGidMap, build_collection, build_faces, build_font_with_gid_map_for};

pub fn ttf_to_woff2(ttf_bytes: &[u8]) -> Result<Vec<u8>, String> {
    ttf2woff2::encode(ttf_bytes, ttf2woff2::BrotliQuality::default())
        .map_err(|e| format!("WOFF2 encoding failed: {e}"))
}
