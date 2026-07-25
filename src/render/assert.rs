use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{Document, DocumentItem, ExpectedGlyph, ShapeFeatureFlag, collect_name_parts, substitute_name_parts};
use crate::issues::{Issue, Severity};
use crate::ref_composite::ResolvedGlyph;
use crate::render::contour::track_contour;
use crate::render::ttf_builder::UNITS_PER_EM;
use crate::pixel::PX_SUBPIXEL;

pub struct AssertShapeResult {
    pub issues: Vec<Issue>,
    pub total: usize,
    pub passed: usize,
}

struct ShapedGlyph {
    glyph_id: u16,
    x_advance: i32,
    x_offset: i32,
    y_offset: i32,
}

/// Shape `text`, splitting it into single-script runs first; see
/// [`crate::script_run`] for why that is required.
fn shape_text(
    font_data: &[u8],
    text: &str,
    features: &[ShapeFeatureFlag],
) -> Vec<ShapedGlyph> {
    crate::script_run::split_script_runs(text)
        .iter()
        .flat_map(|run| shape_run(font_data, &text[run.bytes.clone()], features))
        .collect()
}

fn shape_run(
    font_data: &[u8],
    text: &str,
    features: &[ShapeFeatureFlag],
) -> Vec<ShapedGlyph> {
    let face = harfbuzz_rs::Face::from_bytes(font_data, 0);
    let font = harfbuzz_rs::Font::new(face);
    let buffer = harfbuzz_rs::UnicodeBuffer::new().add_str(text);

    let hb_features: Vec<harfbuzz_rs::Feature> = features
        .iter()
        .map(|f| {
            let bytes = f.tag.as_bytes();
            let mut tag_arr = [b' '; 4];
            for (i, &b) in bytes.iter().enumerate().take(4) {
                tag_arr[i] = b;
            }
            harfbuzz_rs::Feature::new(
                harfbuzz_rs::Tag::new(
                    tag_arr[0] as char, tag_arr[1] as char,
                    tag_arr[2] as char, tag_arr[3] as char,
                ),
                if f.enable { 1 } else { 0 },
                ..,
            )
        })
        .collect();

    let output = harfbuzz_rs::shape(&font, buffer, &hb_features);
    let positions = output.get_glyph_positions();
    let infos = output.get_glyph_infos();

    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, pos)| ShapedGlyph {
            glyph_id: info.codepoint as u16,
            x_advance: pos.x_advance,
            x_offset: pos.x_offset,
            y_offset: pos.y_offset,
        })
        .collect()
}

/// Convert a font-unit offset to pixel units (top-left origin, Y-down).
fn font_units_to_pixel(val: i32, height: u16) -> i32 {
    let upm = UNITS_PER_EM as f32;
    (val as f32 * height as f32 / upm).round() as i32
}

struct CollectedAssertion {
    text: String,
    features: Vec<ShapeFeatureFlag>,
    expected: Vec<ExpectedGlyph>,
    comment: Option<String>,
    file: PathBuf,
    line: usize,
    file_line: usize,
}

fn format_comment_suffix(comment: &Option<String>) -> String {
    match comment {
        Some(c) => format!(" ({c})"),
        None => String::new(),
    }
}

fn collect_assertions(docs: &[&Document]) -> Vec<CollectedAssertion> {
    let mut result = Vec::new();
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            if let DocumentItem::AssertShape { text, features, expected, comment } = item {
                let docline = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
                let file_line = doc.docline_file_lines
                    .get(docline)
                    .copied()
                    .unwrap_or(0) + 1;
                result.push(CollectedAssertion {
                    text: text.clone(),
                    features: features.clone(),
                    expected: expected.clone(),
                    comment: comment.clone(),
                    file: doc.path.clone(),
                    line: docline,
                    file_line,
                });
            }
        }
    }
    result
}

fn run_assertions_inner(
    assertions: Vec<CollectedAssertion>,
    font_data: &[u8],
    gid_to_name: &HashMap<u16, String>,
    height: u16,
) -> AssertShapeResult {
    let total = assertions.len();
    let mut issues = Vec::new();
    let mut passed = 0;

    for assertion in &assertions {
        let shaped = shape_text(font_data, &assertion.text, &assertion.features);

        let got_names: Vec<&str> = shaped
            .iter()
            .map(|g| {
                gid_to_name
                    .get(&g.glyph_id)
                    .map(|s| s.as_str())
                    .unwrap_or("???")
            })
            .collect();

        let expected_names: Vec<&str> = assertion.expected.iter().map(|e| e.name.as_str()).collect();

        if shaped.len() != assertion.expected.len() {
            issues.push(Issue {
                severity: Severity::Error,
                message: format!(
                    "assert shape `{}`{}: expected {} glyph(s) [{}], got {} [{}]",
                    assertion.text,
                    format_comment_suffix(&assertion.comment),
                    assertion.expected.len(),
                    expected_names.join(", "),
                    shaped.len(),
                    got_names.join(", "),
                ),
                file: assertion.file.clone(),
                line: assertion.line,
                file_line: assertion.file_line,
            });
            continue;
        }

        let mut ok = true;
        let mut mismatches = Vec::new();

        for (i, (got, exp)) in shaped.iter().zip(assertion.expected.iter()).enumerate() {
            let got_name = gid_to_name
                .get(&got.glyph_id)
                .map(|s| s.as_str())
                .unwrap_or("???");

            if got_name != exp.name {
                mismatches.push(format!("[{}] name: expected {}, got {}", i, exp.name, got_name));
                ok = false;
            }
            if let Some(adv) = exp.advance {
                if got.x_advance != adv {
                    mismatches.push(format!("[{}] advance: expected {}, got {}", i, adv, got.x_advance));
                    ok = false;
                }
            }
            if let Some((exp_px, exp_py)) = exp.offset {
                let got_px = font_units_to_pixel(got.x_offset, height);
                let got_py = font_units_to_pixel(-got.y_offset, height);
                if got_px != exp_px || got_py != exp_py {
                    mismatches.push(format!(
                        "[{}] offset: expected ({}, {}), got ({}, {})",
                        i, exp_px, exp_py, got_px, got_py,
                    ));
                    ok = false;
                }
            }
        }

        if ok {
            passed += 1;
        } else {
            issues.push(Issue {
                severity: Severity::Error,
                message: format!(
                    "assert shape `{}`{}: {}",
                    assertion.text,
                    format_comment_suffix(&assertion.comment),
                    mismatches.join("; "),
                ),
                file: assertion.file.clone(),
                line: assertion.line,
                file_line: assertion.file_line,
            });
        }
    }

    AssertShapeResult { issues, total, passed }
}

/// Run all shape assertions from all documents.
pub fn run_assertions(
    docs: &[&Document],
    font_data: &[u8],
    gid_to_name: &HashMap<u16, String>,
    height: u16,
) -> AssertShapeResult {
    run_assertions_inner(collect_assertions(docs), font_data, gid_to_name, height)
}

/// Run shape assertions only from the specified subset of documents.
pub fn run_assertions_for_files(
    test_docs: &[&Document],
    font_data: &[u8],
    gid_to_name: &HashMap<u16, String>,
    height: u16,
) -> AssertShapeResult {
    run_assertions_inner(collect_assertions(test_docs), font_data, gid_to_name, height)
}

// ---------------------------------------------------------------------------
// assert same / assert distinct
// ---------------------------------------------------------------------------

/// Canonical contour representation for comparison.
/// Coordinates are in logical-pixel space, quantized to integer lattice
/// points using a common factor `q` so that all glyphs' vertex positions
/// snap to exact integers regardless of their individual `den`/`scale`.
type CanonicalContours = Vec<Vec<(i64, i64)>>;

fn glyph_lattice_denom(g: &ResolvedGlyph) -> i64 {
    2 * g.grid.den.max(1) as i64 * g.scale.max(1) as i64
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 { let t = b; b = a % b; a = t; }
    a
}

fn lcm(a: i64, b: i64) -> i64 {
    a / gcd(a, b) * b
}

fn canonicalize_contours(grid: &crate::document::PixelGrid, scale: u8, q: i64) -> CanonicalContours {
    let raw = track_contour(grid, PX_SUBPIXEL);
    let s = scale.max(1) as f64;
    let mut contours: Vec<Vec<(i64, i64)>> = raw
        .into_iter()
        .filter_map(|path| {
            let quantized: Vec<(i64, i64)> = path
                .iter()
                .map(|&(x, y)| {
                    ((x as f64 / s * q as f64).round() as i64,
                     (y as f64 / s * q as f64).round() as i64)
                })
                .collect();
            let simplified = simplify_collinear(&quantized);
            if simplified.len() < 3 {
                return None;
            }
            Some(rotate_to_min(simplified))
        })
        .collect();
    contours.sort();
    contours
}

fn simplify_collinear(pts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let n = pts.len();
    if n < 3 {
        return pts.to_vec();
    }
    let mut result = Vec::with_capacity(n);
    for i in 0..n {
        let prev = if i == 0 { n - 1 } else { i - 1 };
        let next = (i + 1) % n;
        let (ax, ay) = pts[prev];
        let (bx, by) = pts[i];
        let (cx, cy) = pts[next];
        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if cross != 0 {
            result.push(pts[i]);
        }
    }
    result
}

fn rotate_to_min(mut pts: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    if pts.is_empty() {
        return pts;
    }
    let min_idx = pts
        .iter()
        .enumerate()
        .min_by_key(|&(_, p)| *p)
        .unwrap()
        .0;
    pts.rotate_left(min_idx);
    pts
}

fn glyph_logical_dims(g: &ResolvedGlyph) -> (u16, u16) {
    let s = g.scale.max(1) as u16;
    (g.grid.width / s, g.grid.height / s)
}

struct SameDistinctAssertion {
    is_same: bool,
    names: Vec<String>,
    comment: Option<String>,
    file: PathBuf,
    line: usize,
    file_line: usize,
}

fn collect_same_distinct_assertions(docs: &[&Document]) -> Vec<SameDistinctAssertion> {
    let name_parts = collect_name_parts(docs);
    let mut result = Vec::new();
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let (is_same, names, comment) = match item {
                DocumentItem::AssertSame { names, comment } => (true, names, comment),
                DocumentItem::AssertDistinct { names, comment } => (false, names, comment),
                _ => continue,
            };
            let docline = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
            let file_line = doc.docline_file_lines
                .get(docline)
                .copied()
                .unwrap_or(0) + 1;
            let resolved_names: Vec<String> = names
                .iter()
                .map(|n| substitute_name_parts(n, &name_parts))
                .collect();
            result.push(SameDistinctAssertion {
                is_same,
                names: resolved_names,
                comment: comment.clone(),
                file: doc.path.clone(),
                line: docline,
                file_line,
            });
        }
    }
    result
}

fn run_same_distinct_inner(
    assertions: Vec<SameDistinctAssertion>,
    resolved: &HashMap<String, ResolvedGlyph>,
) -> AssertShapeResult {
    let total = assertions.len();
    let mut issues = Vec::new();
    let mut passed = 0;

    for assertion in &assertions {
        let keyword = if assertion.is_same { "same" } else { "distinct" };

        let mut missing = Vec::new();
        let mut glyphs: Vec<(&str, &ResolvedGlyph)> = Vec::new();
        for name in &assertion.names {
            if let Some(g) = resolved.get(name.as_str()) {
                glyphs.push((name.as_str(), g));
            } else {
                missing.push(name.as_str());
            }
        }

        if !missing.is_empty() {
            issues.push(Issue {
                severity: Severity::Error,
                message: format!(
                    "assert {}{}: undefined glyph(s): {}",
                    keyword,
                    format_comment_suffix(&assertion.comment),
                    missing.join(", "),
                ),
                file: assertion.file.clone(),
                line: assertion.line,
                file_line: assertion.file_line,
            });
            continue;
        }

        let q = glyphs.iter().fold(1i64, |acc, (_, g)| lcm(acc, glyph_lattice_denom(g)));
        let entries: Vec<(&str, (u16, u16), CanonicalContours)> = glyphs
            .iter()
            .map(|(name, g)| {
                let dims = glyph_logical_dims(g);
                let contours = canonicalize_contours(&g.grid, g.scale, q);
                (*name, dims, contours)
            })
            .collect();

        if assertion.is_same {
            let (ref_name, ref_dims, ref_contours) = &entries[0];
            let mut mismatches = Vec::new();
            for (name, dims, contours) in &entries[1..] {
                if dims != ref_dims {
                    mismatches.push(format!(
                        "'{}' ({}x{}) vs '{}' ({}x{}): different dimensions",
                        name, dims.0, dims.1,
                        ref_name, ref_dims.0, ref_dims.1,
                    ));
                } else if contours != ref_contours {
                    mismatches.push(format!(
                        "'{}' vs '{}': different contours",
                        name, ref_name,
                    ));
                }
            }
            if mismatches.is_empty() {
                passed += 1;
            } else {
                issues.push(Issue {
                    severity: Severity::Error,
                    message: format!(
                        "assert same{}: {}",
                        format_comment_suffix(&assertion.comment),
                        mismatches.join("; "),
                    ),
                    file: assertion.file.clone(),
                    line: assertion.line,
                    file_line: assertion.file_line,
                });
            }
        } else {
            let mut duplicates = Vec::new();
            for i in 0..entries.len() {
                for j in (i + 1)..entries.len() {
                    let (ni, di, ci) = &entries[i];
                    let (nj, dj, cj) = &entries[j];
                    if di == dj && ci == cj {
                        duplicates.push(format!("'{}' and '{}'", ni, nj));
                    }
                }
            }
            if duplicates.is_empty() {
                passed += 1;
            } else {
                issues.push(Issue {
                    severity: Severity::Error,
                    message: format!(
                        "assert distinct{}: same rendering: {}",
                        format_comment_suffix(&assertion.comment),
                        duplicates.join("; "),
                    ),
                    file: assertion.file.clone(),
                    line: assertion.line,
                    file_line: assertion.file_line,
                });
            }
        }
    }

    AssertShapeResult { issues, total, passed }
}

/// Run all same/distinct assertions from all documents.
pub fn run_same_distinct_assertions(
    docs: &[&Document],
    resolved: &HashMap<String, ResolvedGlyph>,
) -> AssertShapeResult {
    run_same_distinct_inner(collect_same_distinct_assertions(docs), resolved)
}

/// Run same/distinct assertions only from the specified subset of documents.
pub fn run_same_distinct_assertions_for_files(
    test_docs: &[&Document],
    resolved: &HashMap<String, ResolvedGlyph>,
) -> AssertShapeResult {
    run_same_distinct_inner(collect_same_distinct_assertions(test_docs), resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::collect_name_parts;
    use crate::document_io;
    use crate::ref_composite;

    fn resolve_and_assert(input: &str) -> AssertShapeResult {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs = vec![&doc];
        let name_parts = collect_name_parts(&docs);
        let (resolved, _) = ref_composite::resolve_named_glyphs_with_parts(&docs, &name_parts);
        run_same_distinct_assertions(&docs, &resolved)
    }

    #[test]
    fn assert_same_identical_glyphs_pass() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 2 2
@@@@
..@@

assert same a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn assert_same_different_glyphs_fail() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 2 2
@@..
..@@

assert same a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(result.issues[0].message.contains("different contours"));
    }

    #[test]
    fn assert_same_different_dimensions_fail() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 3 2
@@@@@@
..@@@@

assert same a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(result.issues[0].message.contains("different dimensions"));
    }

    #[test]
    fn assert_distinct_different_glyphs_pass() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 2 2
@@..
..@@

assert distinct a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
        assert!(result.issues.is_empty());
    }

    #[test]
    fn assert_distinct_identical_glyphs_fail() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 2 2
@@@@
..@@

assert distinct a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(result.issues[0].message.contains("same rendering"));
    }

    #[test]
    fn assert_same_alias_matches_original() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b = a

assert same a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
    }

    #[test]
    fn assert_same_composite_matches_pixel() {
        let input = "\
glyph part 2 1
@@@@

glyph composite
ref part 0 0
ref part 0 1

glyph direct 2 2
@@@@
@@@@

assert same composite direct
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
    }

    #[test]
    fn assert_same_undefined_glyph_error() {
        let input = "\
glyph a 2 2
@@@@
..@@

assert same a nonexistent
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(result.issues[0].message.contains("undefined glyph"));
    }

    #[test]
    fn assert_distinct_three_glyphs_pairwise() {
        let input = "\
glyph a 2 1
@@@@

glyph b 2 1
..@@

glyph c 2 1
@@@@

assert distinct a b c
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(result.issues[0].message.contains("'a' and 'c'"));
    }

    #[test]
    fn assert_same_empty_glyphs_pass() {
        let input = "\
glyph a 2 2
....
....

glyph b 2 2
....
....

assert same a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
    }

    #[test]
    fn comment_shown_in_error_message() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 2 2
@@..
..@@

assert same a b // both should be L-shapes
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(
            result.issues[0].message.contains("both should be L-shapes"),
            "error should contain the comment, got: {}",
            result.issues[0].message,
        );
    }

    #[test]
    fn comment_not_treated_as_glyph_name() {
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b 2 2
@@@@
..@@

assert same a b // this is a comment
";
        let result = resolve_and_assert(input);
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
    }

    #[test]
    fn assert_same_fractional_tiled_vs_simple_rect() {
        // Simulates sextant-135 vs left-4-over-8:
        // Three fractional-height blocks (scale 3) tiling the same rectangle
        // as a simple integer-sized ref (scale 1).
        // Parent glyphs must be large enough to contain all refs.
        let input = "\
glyph part-top 8 16 inline
ref 4x5p1r3

glyph part-mid 8 16 inline
ref 4x-0p2r3 0 5
ref 4x4p2r3 0 6

glyph part-bot 8 16 inline
ref 4x-5p1r3 0 10

glyph tiled 8 16
ref part-top
ref part-mid
ref part-bot

glyph simple 8 16
ref 4x16

assert same tiled simple
";
        let result = resolve_and_assert(input);
        assert_eq!(result.passed, 1, "fractional-tiled rect should match simple rect: {:?}",
            result.issues);
    }

    #[test]
    fn assert_same_different_scales_same_shape() {
        // Both glyphs are 2x2 logical pixels, but defined at different scales.
        let input = "\
glyph a 2 2
@@@@
@@@@

glyph b 2 2 scale 2
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

assert same a b
";
        let result = resolve_and_assert(input);
        assert_eq!(result.passed, 1, "different scales, same shape: {:?}",
            result.issues);
    }
}
