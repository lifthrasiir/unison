//! Anchor inheritance, attachment, derivation and centred-class checks; see `crate::issues::anchors`.

use super::*;

#[test]
fn duplicate_inherited_anchors_reported() {
    let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0 inherit
ref half 2 0 inherit
map D = digraph
map h = half
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("digraph")
            && i.message.contains("'+above'")),
        "expected duplicate exposed anchor error, got: {issues:?}",
    );
}

#[test]
fn ambiguous_attachment_reported() {
    let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph mark 2 1 mark
@@@@
anchor -above 0 0
glyph combo
ref half 0 0
ref half 2 0
ref mark
map D = combo
map h = half
map m = mark
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("combo")
            && i.message.contains("'mark'")
            && i.message.contains("'-above'")),
        "expected ambiguous attachment error, got: {issues:?}",
    );
}

/// A `-` anchor that name-matches a published `+` too small to hold it is a
/// near-miss (usually the wrong `:narrow`/`:wide` variant). It is an error,
/// not a note on the side: the mark attached to nothing, so the composite is
/// dropped rather than shipped with the mark at the pen.
#[test]
fn size_mismatched_attachment_reported() {
    let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
glyph mark 2 1 mark
@@@@
anchor -above 0..1 0
glyph combo
ref base
ref mark 1 2
map D = combo
map h = base
map m = mark
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("combo")
            && i.message.contains("'mark'")
            && i.message.contains("'-above'")),
        "expected size-mismatch error, got: {issues:?}",
    );
}

/// The validation pass must resolve an alternative *before* any composite
/// that needs it for size-driven substitution — same guard as the
/// editor's `resolve_expansion` — or it reports a mismatch the real
/// resolution does not have.
#[test]
fn alternative_pending_in_same_round_still_substitutes() {
    let input = "\
glyph circle 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +center 2 1..2

glyph circle:alt
ref circle
anchor +center 2 1

glyph j-inner 2 2
@@@@
@@@@
anchor -center 1 0

glyph j-circled
ref circle
ref j-inner
map j = j-circled
map c = circle
map i = j-inner
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("big enough to hold")),
        "circle:alt must be substituted, got: {issues:?}",
    );
}

/// A digraph without `inherit` exposes nothing — that is the designed
/// fallback, not a problem to report.
#[test]
fn non_inherited_duplicates_are_quiet() {
    let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0
ref half 2 0
map D = digraph
map h = half
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "expected no errors, got: {issues:?}",
    );
}

#[test]
fn duplicate_alternative_anchor_warns() {
    let input = "\
glyph stem 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:a 2 2
@@@@
@@@@
anchor -join 0 0

glyph stem:b 2 2
@@@@
@@@@
anchor -join 0 0
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(
            |i| i.severity == Severity::Warning && i.message.contains("same anchor dimensions")
        ),
        "expected duplicate alternative anchor warning, got: {issues:?}",
    );
}

/// The anchor-derivation check and the font build are two passes over the same
/// graph: the build derives anchors to place its refs and **drops** a glyph
/// whose derivation failed, silently, while
/// [`anchors::check_anchor_derivation`] re-derives with a geometry-free builder
/// to say why. Nothing makes the two agree by construction, so what would
/// otherwise be a silent divergence — a glyph missing from the font that no
/// issue accounts for — is pinned here instead.
#[test]
fn a_faulted_anchor_derivation_is_a_glyph_the_build_drops() {
    let input = "\
glyph half 2 2
@@@@
@@@@
anchor +above 1 0
glyph digraph
ref half 0 0 inherit
ref half 2 0 inherit
map D = digraph
map h = half
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    assert!(
        collect_issues(&[&doc])
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("digraph")),
        "the check has to fault the glyph",
    );
    let built =
        crate::render::ttf_builder::build_font_with_gid_map(&[&doc]).expect("font should build");
    assert!(
        !built.gid_to_name.values().any(|n| n == "digraph"),
        "and the build has to drop exactly that glyph: {:?}",
        built.gid_to_name.values().collect::<Vec<_>>(),
    );
    assert!(
        built.gid_to_name.values().any(|n| n == "half"),
        "while a glyph nothing faulted is still built",
    );
}

/// Centring reduces both sides of a pairing to their middles, so the offset is
/// half the difference of the two sizes — a whole pixel only when the sizes
/// share a parity. The odd case is silent otherwise: the mark lands half a
/// pixel off, which the bitmap face cannot draw at all.
#[test]
fn a_centred_anchor_class_warns_when_the_two_sizes_disagree_in_parity() {
    let source = |slot: &str| {
        format!(
            "\
feature ccmp for DFLT : anchor slot align c

glyph base 8 1
................
anchor +slot {slot} 0

glyph tick 8 1 mark advance 0
................
anchor -slot 3..5 0
"
        )
    };

    // 4-wide slot against a 3-wide mark: half a pixel.
    let issues = issues_for(&source("1..4"));
    assert!(
        has(&issues, Severity::Warning, "lands half a pixel off"),
        "a parity mismatch under `align c` must be reported, got {:?}",
        issues.iter().map(|i| &i.message).collect::<Vec<_>>()
    );

    // 7-wide slot against the same 3-wide mark: two whole pixels each side.
    let issues = issues_for(&source("1..7"));
    assert!(
        !has(&issues, Severity::Warning, "lands half a pixel off"),
        "matching parity must not be reported, got {:?}",
        issues.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
}

/// The default reduction takes the low end of each range, where no parity
/// question arises — the check must not fire on a class that never centres.
#[test]
fn an_uncentred_anchor_class_is_never_held_to_the_parity_rule() {
    let issues = issues_for(
        "\
feature ccmp for DFLT : anchor slot

glyph base 8 1
................
anchor +slot 1..4 0

glyph tick 8 1 mark advance 0
................
anchor -slot 3..5 0
",
    );
    assert!(!has(&issues, Severity::Warning, "lands half a pixel off"));
}

/// A `scale 2` mark whose `-dot` sits half a cell across, and a combo of it
/// with a `scale 1` base at the given `scale`.
fn scaled_dot_combo(combo_scale: u8) -> String {
    format!(
        "\
feature ccmp for DFLT : anchor dot

glyph base 4 4
........
........
........
........
anchor +dot 2 1

glyph dot 2 4 mark scale 2
........
........
........
........
........
........
........
........
anchor -dot 1..2 2..3

glyph combo scale {combo_scale}
ref base
ref dot
"
    )
}

/// An anchor's *position* may fall between declared cells — that is what a
/// `scale` is for — but a ref offset is whole cells of the glyph that writes
/// it. A mark that attaches half a cell across therefore fits a `scale 2`
/// composite and not a `scale 1` one, and the latter must say so rather than
/// drop the half cell.
#[test]
fn an_attachment_finer_than_the_composites_grid_is_an_error() {
    let fine = issues_for(&scaled_dot_combo(2));
    assert!(
        !fine.iter().any(|i| i.severity == Severity::Error),
        "a `scale 2` composite holds a half-cell offset, got {:?}",
        fine.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
    let coarse = issues_for(&scaled_dot_combo(1));
    assert!(
        has(&coarse, Severity::Error, "'combo'") && has(&coarse, Severity::Error, "scale"),
        "a `scale 1` composite cannot, got {:?}",
        coarse.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
}

/// An anchor's *size* is in declared cells, so on a `scale N` glyph it has to
/// be a whole multiple of N fine cells on both axes; one fine cell of a
/// `scale 2` glyph is no size a `scale 1` glyph can match.
#[test]
fn an_anchor_size_that_is_not_whole_declared_cells_is_an_error() {
    let issues = issues_for(
        "\
glyph dot 2 2 scale 2
........
........
........
........
anchor -dot 1 0..1
anchor +dot 0..1 1..2
",
    );
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error)
        .map(|i| &i.message)
        .collect();
    assert_eq!(errors.len(), 1, "only `-dot` is 1×2 fine cells: {errors:?}");
    assert!(errors[0].contains("'-dot'"), "{errors:?}");
}
