//! The `assert` directives: shaping assertions run against the built font, and
//! resolved-glyph equality assertions.
//!
//! `assert shape` shapes its text with rustybuzz and compares the result glyph
//! by glyph, so it is the font's own regression suite (`uniform test`, `make
//! test`). Its `@lang` is a **BCP 47** tag (`@ro`), deliberately not the
//! `script/LANG` language system a `feature` directive declares: an assertion
//! states the input a real client hands the shaper, and deriving the OpenType
//! language system from it is the shaper's job — part of what is under test.
//! Writing `@ROM` on both sides would make the two agree by construction and
//! stop the assertion from ever noticing that Romanian text never reaches the
//! declared tag.
//!
//! `for SLICE...` is likewise part of the assertion, not a label: an unqualified
//! assertion is shaped against the primary face, and a slice-scoped one against
//! *every* face that includes those slices, each from its own build. A face has
//! its own cmap and its own GSUB, so shaping `for narrow` against the primary
//! face would test the wrong font and quietly agree with itself.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::document::{
    Document, DocumentItem, ExpectedGlyph, ShapeFeatureFlag, collect_name_parts,
    substitute_name_parts,
};
use crate::issues::{Issue, Severity};
use crate::pixel::PX_SUBPIXEL;
use crate::ref_composite::ResolvedGlyph;
use crate::render::contour::track_contour;
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

/// Shape `text`, splitting it into single-script runs first; see
/// [`crate::script_run`] for why that is required.
fn shape_text(
    font_data: &[u8],
    text: &str,
    features: &[ShapeFeatureFlag],
    language: Option<&str>,
) -> Vec<ShapedGlyph> {
    crate::script_run::split_script_runs(text)
        .iter()
        .flat_map(|run| shape_run(font_data, &text[run.bytes.clone()], features, language))
        .collect()
}

fn shape_run(
    font_data: &[u8],
    text: &str,
    features: &[ShapeFeatureFlag],
    language: Option<&str>,
) -> Vec<ShapedGlyph> {
    let Some(face) = rustybuzz::Face::from_slice(font_data, 0) else {
        return Vec::new();
    };
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(text);
    // The shaper turns a BCP 47 language into an OpenType language system
    // itself; an unparseable tag is left to the assertion to fail on, which
    // says more than silently shaping language-neutral would.
    if let Some(language) = language
        && let Ok(language) = language.parse()
    {
        buffer.set_language(language);
    }

    let hb_features: Vec<rustybuzz::Feature> = features
        .iter()
        .map(|f| {
            rustybuzz::Feature::new(
                feature_tag(f.tag.as_bytes()),
                if f.enable { 1 } else { 0 },
                ..,
            )
        })
        .collect();

    let output = rustybuzz::shape(&face, &hb_features, buffer);
    let positions = output.glyph_positions();
    let infos = output.glyph_infos();

    infos
        .iter()
        .zip(positions.iter())
        .map(|(info, pos)| ShapedGlyph {
            glyph_id: info.glyph_id as u16,
            x_advance: pos.x_advance,
            x_offset: pos.x_offset,
            y_offset: pos.y_offset,
        })
        .collect()
}

/// OpenType tags are exactly four bytes; shorter names are space-padded.
pub(crate) fn feature_tag(bytes: &[u8]) -> rustybuzz::ttf_parser::Tag {
    let mut tag = [b' '; 4];
    for (slot, &b) in tag.iter_mut().zip(bytes.iter()) {
        *slot = b;
    }
    rustybuzz::ttf_parser::Tag::from_bytes(&tag)
}

/// Convert a font-unit offset to pixel units (top-left origin, Y-down).
fn font_units_to_pixel(val: i32, height: u16) -> i32 {
    let upm = UNITS_PER_EM as f32;
    (val as f32 * height as f32 / upm).round() as i32
}

struct CollectedAssertion {
    slices: Vec<String>,
    text: String,
    features: Vec<ShapeFeatureFlag>,
    language: Option<String>,
    expected: Vec<ExpectedGlyph>,
    comment: Option<String>,
    file: PathBuf,
    line: usize,
    file_line: usize,
}

/// How a failing assertion names itself: the text, then the language it was
/// shaped as, so a Romanian-only failure is not mistaken for a general one.
fn format_subject(assertion: &CollectedAssertion, face_id: &str) -> String {
    let mut s = match &assertion.language {
        Some(lang) => format!("`{}` @{lang}", assertion.text),
        None => format!("`{}`", assertion.text),
    };
    // Only a slice-scoped assertion runs on more than the primary face, so
    // naming the face is what tells two failures of the same line apart.
    if !assertion.slices.is_empty() {
        s.push_str(&format!(
            " [{}]",
            if face_id.is_empty() {
                "<default>"
            } else {
                face_id
            }
        ));
    }
    s
}

fn format_comment_suffix(comment: &Option<String>) -> String {
    match comment {
        Some(c) => format!(" ({c})"),
        None => String::new(),
    }
}

fn collect_assertions(docs: &[&Document]) -> Vec<CollectedAssertion> {
    // An assertion names glyphs to compare against the *built* font's names,
    // and an alias has no name of its own there — so writing one is the same
    // as writing its target. See [`crate::alias`].
    let name_parts = collect_name_parts(docs);
    let (exists, _) = crate::exists::resolve_scopes(
        docs,
        &name_parts,
        &crate::alias::AliasMap::collect(docs, &name_parts),
    );
    let aliases = crate::alias::AliasMap::collect_with_merges(docs, &name_parts, &exists);

    let mut result = Vec::new();
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            if let DocumentItem::AssertShape {
                slices,
                text,
                features,
                language,
                expected,
                comment,
            } = item
            {
                let (docline, file_line) = doc.item_lines(item_idx);
                let mut expected = expected.clone();
                for e in &mut expected {
                    aliases.canonicalize(&mut e.name);
                }
                result.push(CollectedAssertion {
                    slices: slices.clone(),
                    text: text.clone(),
                    features: features.clone(),
                    language: language.clone(),
                    expected,
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

/// Builds one face on demand. Every caller has its own way to get there — the
/// CLI builds from scratch, the editor goes through its warm contour cache — and
/// the runner only cares that it is asked for a face at most once.
pub type FaceBuilder<'a> =
    &'a mut dyn FnMut(&crate::faces::Face) -> Option<crate::render::FontWithGidMap>;

/// Run every assertion, each against the face(s) its `for SLICE...` names.
///
/// An unqualified assertion means the primary face; a slice-scoped one is a
/// statement about the faces that include those slices, so it is shaped against
/// each of them — with its own build, since a face has its own cmap and its own
/// GSUB and thus its own glyph ids.
///
/// Every build goes through `build_face`, memoized here by face id, and nothing
/// is built until an assertion actually needs it. **A source, or a file, with no
/// shape assertions therefore builds no font at all** — which is the difference
/// between the editor's per-file test finishing instantly and its costing three
/// full builds.
fn run_assertions_inner(
    assertions: Vec<CollectedAssertion>,
    docs: &[&Document],
    build_face: FaceBuilder<'_>,
) -> AssertShapeResult {
    let mut total = 0;
    let mut issues = Vec::new();
    let mut passed = 0;

    if assertions.is_empty() {
        return AssertShapeResult {
            issues,
            total,
            passed,
        };
    }

    let faces = crate::faces::FaceSet::collect(docs);
    let mut face_fonts: HashMap<String, Option<crate::render::FontWithGidMap>> = HashMap::new();

    for assertion in &assertions {
        // An assertion no face satisfies is reported by `issues.rs`; here it
        // simply contributes nothing to run.
        let targets: Vec<&crate::faces::Face> = if assertion.slices.is_empty() {
            vec![faces.primary()]
        } else {
            faces
                .faces
                .iter()
                .filter(|f| f.includes_all(&assertion.slices))
                .collect()
        };

        for face in targets {
            if !face_fonts.contains_key(&face.id) {
                let built = build_face(face);
                face_fonts.insert(face.id.clone(), built);
            }
            // An unqualified assertion names no face and is reported without
            // one, even though it ran against the primary face's build.
            let label = if assertion.slices.is_empty() {
                ""
            } else {
                face.id.as_str()
            };
            total += 1;
            let Some(built) = face_fonts.get(&face.id).and_then(|b| b.as_ref()) else {
                issues.push(Issue {
                    glyph: None,
                    severity: Severity::Error,
                    message: format!(
                        "assert shape {}: face failed to build",
                        format_subject(assertion, label),
                    ),
                    file: assertion.file.clone(),
                    line: assertion.line,
                    file_line: assertion.file_line,
                });
                continue;
            };
            match check_assertion(
                assertion,
                label,
                &built.ttf,
                &built.gid_to_name,
                built.height,
            ) {
                Some(issue) => issues.push(issue),
                None => passed += 1,
            }
        }
    }

    AssertShapeResult {
        issues,
        total,
        passed,
    }
}

/// Shape one assertion against one built face; `None` means it passed.
fn check_assertion(
    assertion: &CollectedAssertion,
    face_id: &str,
    font_data: &[u8],
    gid_to_name: &HashMap<u16, String>,
    height: u16,
) -> Option<Issue> {
    let shaped = shape_text(
        font_data,
        &assertion.text,
        &assertion.features,
        assertion.language.as_deref(),
    );

    let issue = |message: String| Issue {
        glyph: None,
        severity: Severity::Error,
        message,
        file: assertion.file.clone(),
        line: assertion.line,
        file_line: assertion.file_line,
    };

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
        return Some(issue(format!(
            "assert shape {}{}: expected {} glyph(s) [{}], got {} [{}]",
            format_subject(assertion, face_id),
            format_comment_suffix(&assertion.comment),
            assertion.expected.len(),
            expected_names.join(", "),
            shaped.len(),
            got_names.join(", "),
        )));
    }

    let mut mismatches = Vec::new();

    for (i, (got, exp)) in shaped.iter().zip(assertion.expected.iter()).enumerate() {
        let got_name = gid_to_name
            .get(&got.glyph_id)
            .map(|s| s.as_str())
            .unwrap_or("???");

        if got_name != exp.name {
            mismatches.push(format!(
                "[{}] name: expected {}, got {}",
                i, exp.name, got_name
            ));
        }
        if let Some(adv) = exp.advance
            && got.x_advance != adv
        {
            mismatches.push(format!(
                "[{}] advance: expected {}, got {}",
                i, adv, got.x_advance
            ));
        }
        if let Some((exp_px, exp_py)) = exp.offset {
            let got_px = font_units_to_pixel(got.x_offset, height);
            let got_py = font_units_to_pixel(-got.y_offset, height);
            if got_px != exp_px || got_py != exp_py {
                mismatches.push(format!(
                    "[{}] offset: expected ({}, {}), got ({}, {})",
                    i, exp_px, exp_py, got_px, got_py,
                ));
            }
        }
    }

    if mismatches.is_empty() {
        return None;
    }
    Some(issue(format!(
        "assert shape {}{}: {}",
        format_subject(assertion, face_id),
        format_comment_suffix(&assertion.comment),
        mismatches.join("; "),
    )))
}

/// Run all shape assertions from all documents.
pub fn run_assertions(docs: &[&Document], build_face: FaceBuilder<'_>) -> AssertShapeResult {
    run_assertions_inner(collect_assertions(docs), docs, build_face)
}

/// Run shape assertions only from the specified subset of documents.
///
/// `docs` is still the *whole* source: which faces exist, and what each of them
/// contains, is a property of the font, not of the file being edited.
#[cfg(feature = "editor")]
pub fn run_assertions_for_files(
    test_docs: &[&Document],
    docs: &[&Document],
    build_face: FaceBuilder<'_>,
) -> AssertShapeResult {
    run_assertions_inner(collect_assertions(test_docs), docs, build_face)
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

fn canonicalize_contours(
    grid: &crate::document::PixelGrid,
    scale: u8,
    q: i64,
) -> CanonicalContours {
    let raw = track_contour(grid, PX_SUBPIXEL);
    let s = scale.max(1) as f64;
    let mut contours: Vec<Vec<(i64, i64)>> = raw
        .into_iter()
        .filter_map(|path| {
            let quantized: Vec<(i64, i64)> = path
                .iter()
                .map(|&(x, y)| {
                    (
                        (x as f64 / s * q as f64).round() as i64,
                        (y as f64 / s * q as f64).round() as i64,
                    )
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
    let min_idx = pts.iter().enumerate().min_by_key(|&(_, p)| *p).unwrap().0;
    pts.rotate_left(min_idx);
    pts
}

/// Logical dimensions as exact reduced rationals `(numerator, denominator)`,
/// one per axis. A composite's raster size need not be a multiple of its
/// scale, and integer division would collapse sizes that differ by less than
/// one scale unit (9/4 vs 8/4 both "2"), letting `assert distinct` call two
/// such glyphs the same.
fn glyph_logical_dims(g: &ResolvedGlyph) -> ((u32, u32), (u32, u32)) {
    let s = g.scale.max(1) as u32;
    let reduce = |n: u32| {
        let d = crate::pattern::gcd(n as usize, s as usize).max(1) as u32;
        (n / d, s / d)
    };
    (reduce(g.grid.width as u32), reduce(g.grid.height as u32))
}

fn fmt_dim((n, d): (u32, u32)) -> String {
    if d == 1 {
        n.to_string()
    } else {
        format!("{n}/{d}")
    }
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
            let (docline, file_line) = doc.item_lines(item_idx);
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
        let keyword = if assertion.is_same {
            "same"
        } else {
            "distinct"
        };

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
                glyph: None,
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

        let q = glyphs.iter().fold(1i64, |acc, (_, g)| {
            crate::pattern::lcm(acc as usize, glyph_lattice_denom(g) as usize) as i64
        });
        let entries: Vec<_> = glyphs
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
                        name,
                        fmt_dim(dims.0),
                        fmt_dim(dims.1),
                        ref_name,
                        fmt_dim(ref_dims.0),
                        fmt_dim(ref_dims.1),
                    ));
                } else if contours != ref_contours {
                    mismatches.push(format!("'{}' vs '{}': different contours", name, ref_name,));
                }
            }
            if mismatches.is_empty() {
                passed += 1;
            } else {
                issues.push(Issue {
                    glyph: None,
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
                    glyph: None,
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

    AssertShapeResult {
        issues,
        total,
        passed,
    }
}

/// Run all same/distinct assertions from all documents.
pub fn run_same_distinct_assertions(
    docs: &[&Document],
    resolved: &HashMap<String, ResolvedGlyph>,
) -> AssertShapeResult {
    run_same_distinct_inner(collect_same_distinct_assertions(docs), resolved)
}

/// Whether any of `docs` states an `assert same` or `assert distinct`.
///
/// These need every glyph resolved, which costs about as much as a font build,
/// so a caller that would have to compute that resolution asks first. Scanning
/// the items is free next to it.
#[cfg(feature = "editor")]
pub fn has_same_distinct_assertions(docs: &[&Document]) -> bool {
    docs.iter().any(|doc| {
        doc.items.iter().any(|item| {
            matches!(
                item,
                DocumentItem::AssertSame { .. } | DocumentItem::AssertDistinct { .. }
            )
        })
    })
}

/// Run same/distinct assertions only from the specified subset of documents.
#[cfg(feature = "editor")]
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

    /// A two-face source where `A` is a different glyph in each face, so an
    /// assertion running against the wrong face is visible as a name mismatch.
    const TWO_FACES: &str = "\
meta height 2
meta ascent 2
meta descent 0

slice wide
slice narrow
face regular : wide
face term : narrow

glyph a-wide 2 2
@@@@
@@@@

glyph a-narrow 1 2
@@
@@

map wide : A = a-wide
map narrow : A = a-narrow
";

    fn shape_assert(input: &str) -> AssertShapeResult {
        shape_assert_counting_builds(input).0
    }

    /// The same, also reporting which faces were built and how many times. A
    /// face build is the whole cost of running the assertions, so what gets
    /// built is the thing worth asserting about.
    fn shape_assert_counting_builds(input: &str) -> (AssertShapeResult, Vec<String>) {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let docs = vec![&doc];
        let mut built_faces = Vec::new();
        let result = run_assertions(&docs, &mut |face| {
            built_faces.push(face.id.clone());
            crate::render::build_font_with_gid_map_for(&docs, face)
        });
        (result, built_faces)
    }

    /// A source with no `assert shape` at all builds nothing. The editor runs
    /// the assertions of one file at a time, and most files state none; it used
    /// to build the whole font anyway before finding that out.
    #[test]
    fn no_shape_assertion_builds_no_face() {
        let (result, built) = shape_assert_counting_builds(TWO_FACES);
        assert_eq!(result.total, 0);
        assert!(built.is_empty(), "built {built:?}");
    }

    /// The primary face is built once even when both an unqualified assertion
    /// and one scoped to a slice it contains ask for it. The unqualified one
    /// used to arrive as a separate pre-built font, so `regular` was built
    /// twice — a second full build for nothing.
    #[test]
    fn the_primary_face_is_built_once_for_qualified_and_unqualified_assertions() {
        let (result, built) = shape_assert_counting_builds(&format!(
            "{TWO_FACES}\n\
             assert shape A : a-wide\n\
             assert shape A for wide : a-wide\n\
             assert shape A for wide : a-wide\n"
        ));
        assert_eq!(result.total, 3);
        assert_eq!(result.passed, 3, "{:?}", result.issues);
        assert_eq!(built, vec!["regular".to_string()]);
    }

    /// Each face is built at most once however many assertions reach it.
    #[test]
    fn each_face_is_built_at_most_once() {
        let (result, mut built) = shape_assert_counting_builds(&format!(
            "{TWO_FACES}\n\
             assert shape A for wide : a-wide\n\
             assert shape A for narrow : a-narrow\n\
             assert shape A for narrow : a-narrow\n"
        ));
        assert_eq!(result.passed, 3, "{:?}", result.issues);
        built.sort();
        assert_eq!(built, vec!["regular".to_string(), "term".to_string()]);
    }

    /// `for SLICE` picks the face the assertion is shaped against. Without it,
    /// every assertion silently runs against the primary face, so a `for narrow`
    /// assertion would test `regular` and report the wide glyph.
    #[test]
    fn assert_shape_runs_against_the_face_its_slice_names() {
        let result = shape_assert(&format!(
            "{TWO_FACES}\nassert shape A for narrow : a-narrow\n"
        ));
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1, "{:?}", result.issues[0].message);
    }

    /// The primary face is still what an unqualified assertion means.
    #[test]
    fn assert_shape_without_a_slice_uses_the_primary_face() {
        let result = shape_assert(&format!("{TWO_FACES}\nassert shape A : a-wide\n"));
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 1);
    }

    /// A slice-scoped assertion that is wrong for its face must still fail —
    /// otherwise the fix above would just make every such assertion pass.
    #[test]
    fn assert_shape_for_a_slice_still_fails_when_wrong() {
        let result = shape_assert(&format!(
            "{TWO_FACES}\nassert shape A for narrow : a-wide\n"
        ));
        assert_eq!(result.total, 1);
        assert_eq!(result.passed, 0);
        assert!(
            result.issues[0].message.contains("term"),
            "{}",
            result.issues[0].message
        );
    }

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
    fn assert_same_ref_copy_matches_original() {
        // A `ref` copy, not `glyph b = a`: an alias is the *same* glyph, so
        // asserting it same as its target would hold by construction and
        // check nothing.
        let input = "\
glyph a 2 2
@@@@
..@@

glyph b
ref a

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

    /// Raster widths 9 and 8 at scale 4 are logically 9/4 vs 2. Integer
    /// division used to call both "2", so two blank glyphs of those widths
    /// compared equal and `assert distinct` wrongly failed on them.
    #[test]
    fn assert_distinct_sees_a_sub_scale_width_difference() {
        use crate::document::PixelGrid;
        let make = |w: u16| ResolvedGlyph {
            grid: PixelGrid::new(w, 8),
            origin_row: 0,
            origin_col: 0,
            resolved_anchors: vec![],
            declared_anchors: vec![],
            scale: 4,
            declared_box: None,
            declared_origin: (0, 0),
            inline_source: None,
        };
        let mut resolved = HashMap::new();
        resolved.insert("a".to_string(), make(9));
        resolved.insert("b".to_string(), make(8));
        let assertion = SameDistinctAssertion {
            is_same: false,
            names: vec!["a".to_string(), "b".to_string()],
            comment: None,
            file: "test.unf".into(),
            line: 1,
            file_line: 1,
        };
        let result = run_same_distinct_inner(vec![assertion], &resolved);
        assert_eq!(result.passed, 1, "{:?}", result.issues);
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
        assert_eq!(
            result.passed, 1,
            "fractional-tiled rect should match simple rect: {:?}",
            result.issues
        );
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
        assert_eq!(
            result.passed, 1,
            "different scales, same shape: {:?}",
            result.issues
        );
    }
}
