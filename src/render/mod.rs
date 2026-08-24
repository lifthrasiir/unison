pub mod assert;
pub mod contour;
pub mod demo;
pub(crate) mod glyph_cache;
pub mod sample;
pub mod ttf_builder;

#[cfg(any(feature = "editor", test))]
pub use ttf_builder::build_font_from_documents;
#[cfg(feature = "editor")]
pub use ttf_builder::{
    BuiltFontPair, SharedContourCache, build_font_pair_cached, build_font_pair_cached_from,
    build_font_with_gid_map_for_cached, new_contour_cache,
};
pub use ttf_builder::{
    FontWithGidMap, build_collection, build_face_ttf_pair, build_faces_from,
    build_font_with_gid_map_for,
};

/// How hard the WOFF2 encoder compresses.
///
/// Brotli's top two levels are where nearly all of a `build`'s remaining wall
/// clock goes, and the curve is very lopsided. Measured on `unison-regular`:
///
/// | quality | size | time |
/// | --- | --- | --- |
/// | 9 (`Fast`) | 249,828 | 0.07 s |
/// | 10 | 227,108 | 0.79 s |
/// | 11 (`Max`) | 220,712 | 1.48 s |
///
/// So the published font is compressed at 11 and nothing else is: a local
/// edit-build loop pays a second and a half per face for 13% of a file it does
/// not serve. `Fast` is the default for that reason, and the Pages workflow
/// passes `--woff2-quality max` for the files it actually publishes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Woff2Quality {
    #[default]
    Fast,
    Max,
}

impl Woff2Quality {
    /// Parses the `--woff2-quality` argument; `None` for a word that is neither.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "fast" => Some(Self::Fast),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

pub fn ttf_to_woff2(ttf_bytes: &[u8], quality: Woff2Quality) -> Result<Vec<u8>, String> {
    let q = match quality {
        Woff2Quality::Fast => 9u8,
        Woff2Quality::Max => 11u8,
    };
    ttf2woff2::encode(ttf_bytes, ttf2woff2::BrotliQuality::from(q))
        .map_err(|e| format!("WOFF2 encoding failed: {e}"))
}
