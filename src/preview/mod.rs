pub mod cluster;
pub mod harfbuzz;
pub mod rasterizer;
pub mod widget;

#[cfg(target_os = "macos")]
pub mod coretext;

#[cfg(target_os = "windows")]
pub mod directwrite;

/// Maps each UTF-16 code unit index of `text` to its char index.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_utf16_to_char_map(text: &str) -> Vec<usize> {
    let mut map = Vec::new();
    for (char_idx, ch) in text.chars().enumerate() {
        for _ in 0..ch.len_utf16() {
            map.push(char_idx);
        }
    }
    map
}

#[derive(Clone, Debug)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub cluster: usize,
    pub x_advance: f32,
    #[allow(dead_code)]
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug)]
pub struct ShapeError(pub String);

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait ShaperBackend {
    fn name(&self) -> &'static str;
    fn shape(
        &self,
        font_data: &[u8],
        text: &str,
        upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError>;
}

/// Shape `text` with `backend`, splitting it into single-script runs first and
/// concatenating the results; see [`crate::script_run`] for why that is
/// required. Cluster indices stay relative to the whole text.
pub fn shape_text(
    backend: &dyn ShaperBackend,
    font_data: &[u8],
    text: &str,
    upm: u16,
    features: &[Feature],
) -> Result<Vec<ShapedGlyph>, ShapeError> {
    let runs = crate::script_run::split_script_runs(text);
    if runs.len() <= 1 {
        return backend.shape(font_data, text, upm, features);
    }

    let mut glyphs = Vec::new();
    for run in runs {
        let run_text = &text[run.bytes.clone()];
        let run_features: Vec<Feature> = features
            .iter()
            .filter_map(|f| clip_feature(f, run.char_start, run_text.chars().count()))
            .collect();
        for mut glyph in backend.shape(font_data, run_text, upm, &run_features)? {
            glyph.cluster += run.char_start;
            glyphs.push(glyph);
        }
    }
    Ok(glyphs)
}

/// Restrict a feature's character range to a run and rebase it onto that run.
/// Returns `None` if the feature does not overlap the run at all.
fn clip_feature(feature: &Feature, char_start: usize, char_len: usize) -> Option<Feature> {
    let start = feature.start.max(char_start);
    let end = feature.end.min(char_start + char_len);
    (start < end).then(|| Feature {
        tag: feature.tag,
        value: feature.value,
        start: start - char_start,
        end: end - char_start,
    })
}

pub fn available_backends() -> Vec<Box<dyn ShaperBackend>> {
    let mut v: Vec<Box<dyn ShaperBackend>> = vec![Box::new(harfbuzz::HarfBuzzBackend)];
    #[cfg(target_os = "macos")]
    v.push(Box::new(coretext::CoreTextBackend));
    #[cfg(target_os = "windows")]
    v.push(Box::new(directwrite::DirectWriteBackend));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Stands in for a real shaper: emits one glyph per character, with the
    /// cluster index relative to the text it was handed, and records the texts
    /// it was called with.
    struct RecordingBackend(RefCell<Vec<String>>);

    impl ShaperBackend for RecordingBackend {
        fn name(&self) -> &'static str {
            "recording"
        }

        fn shape(
            &self,
            _font_data: &[u8],
            text: &str,
            _upm: u16,
            _features: &[Feature],
        ) -> Result<Vec<ShapedGlyph>, ShapeError> {
            self.0.borrow_mut().push(text.to_string());
            Ok(text
                .chars()
                .enumerate()
                .map(|(i, _)| ShapedGlyph {
                    glyph_id: 0,
                    cluster: i,
                    x_advance: 1.0,
                    y_advance: 0.0,
                    x_offset: 0.0,
                    y_offset: 0.0,
                })
                .collect())
        }
    }

    fn shape_recording(text: &str) -> (Vec<String>, Vec<usize>) {
        let backend = RecordingBackend(RefCell::new(Vec::new()));
        let glyphs = shape_text(&backend, &[], text, 1024, &[]).unwrap();
        (backend.0.into_inner(), glyphs.iter().map(|g| g.cluster).collect())
    }

    #[test]
    fn single_script_text_is_shaped_in_one_call() {
        let (texts, clusters) = shape_recording("가나다");
        assert_eq!(texts, vec!["가나다"]);
        assert_eq!(clusters, vec![0, 1, 2]);
    }

    #[test]
    fn mixed_script_text_is_shaped_per_run() {
        let (texts, _) = shape_recording("ひ 각\u{302F}");
        assert_eq!(texts, vec!["ひ ", "각\u{302F}"]);
    }

    #[test]
    fn clusters_are_rebased_onto_the_whole_text() {
        // Runs are "ひ " (chars 0..2) and "각\u{302F}" (chars 2..4).
        let (_, clusters) = shape_recording("ひ 각\u{302F}");
        assert_eq!(clusters, vec![0, 1, 2, 3]);
    }

    #[test]
    fn features_are_clipped_and_rebased_per_run() {
        let feature = Feature { tag: *b"liga", value: 1, start: 1, end: 3 };
        // Whole-text range 1..3 covers the tail of run 0 and the head of run 1.
        assert_eq!(
            clip_feature(&feature, 0, 2).map(|f| (f.start, f.end)),
            Some((1, 2)),
        );
        assert_eq!(
            clip_feature(&feature, 2, 2).map(|f| (f.start, f.end)),
            Some((0, 1)),
        );
        // A run past the feature's range gets nothing.
        assert!(clip_feature(&feature, 4, 2).is_none());
    }

    #[test]
    fn whole_text_feature_range_survives_clipping() {
        let feature = Feature { tag: *b"kern", value: 1, start: 0, end: usize::MAX };
        assert_eq!(
            clip_feature(&feature, 2, 2).map(|f| (f.start, f.end)),
            Some((0, 2)),
        );
    }
}
