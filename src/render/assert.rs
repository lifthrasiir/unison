use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{Document, DocumentItem, ExpectedGlyph, ShapeFeatureFlag};
use crate::issues::{Issue, Severity};
use crate::render::ttf_builder::UNITS_PER_EM;

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

fn shape_text(
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
    file: PathBuf,
    line: usize,
    file_line: usize,
}

fn collect_assertions(docs: &[&Document]) -> Vec<CollectedAssertion> {
    let mut result = Vec::new();
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            if let DocumentItem::AssertShape { text, features, expected } = item {
                let docline = doc.item_line_starts.get(item_idx).copied().unwrap_or(0);
                let file_line = doc.docline_file_lines
                    .get(docline)
                    .copied()
                    .unwrap_or(0) + 1;
                result.push(CollectedAssertion {
                    text: text.clone(),
                    features: features.clone(),
                    expected: expected.clone(),
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
                    "assert shape `{}`: expected {} glyph(s) [{}], got {} [{}]",
                    assertion.text,
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
                    "assert shape `{}`: {}",
                    assertion.text,
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
