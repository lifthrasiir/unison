//! Tests for [`crate::issues`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;
use crate::document_io;

fn ragged_messages(input: &str) -> Vec<String> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.severity == Severity::Warning && i.message.contains("does not divide"))
        .map(|i| i.message)
        .collect()
}

/// Groups now combine by the largest one, so a group that does not divide
/// it repeats partway and drops combinations. That is nearly always a typo,
/// and nothing downstream can tell it from a deliberate cycle.
#[test]
fn a_ragged_group_in_a_glyph_name_is_a_warning() {
    let msgs = ragged_messages(
        "\
glyph pix 1 1
@@
glyph out-(a|b)-(1|2|3)
ref pix
",
    );
    assert_eq!(msgs.len(), 1, "expected one warning, got: {msgs:?}");
    assert!(
        msgs[0].contains("out-(a|b)-(1|2|3)") && msgs[0].contains("**"),
        "the warning must name the pattern and point at `**N`: {msgs:?}",
    );
}

/// The cross product spelled with `**N`, and an evenly tiling group, are
/// both what the lock-step rule is for — neither may warn.
#[test]
fn an_evenly_dividing_group_is_not_ragged() {
    assert!(
        ragged_messages(
            "\
glyph pix 1 1
@@
glyph out-(a|b**3)-(1|2|3)
ref pix
glyph even-(a|b)-(1|2|3|4)
ref pix
glyph plain
ref pix
"
        )
        .is_empty(),
    );
}

/// Across a remap's operands the same rule holds: the entry count is the
/// longest operand, and a shorter one has to tile it.
#[test]
fn a_ragged_remap_operand_is_a_warning() {
    let msgs = ragged_messages(
        "\
glyph (a|b|c|d|e) 1 1
@@
map (A|B|C|D|E) = (a|b|c|d|e)
remap liga : (a|b) -> (c|d|e)
feature liga for DFLT : liga
",
    );
    assert_eq!(msgs.len(), 1, "expected one warning, got: {msgs:?}");
    assert!(
        msgs[0].contains("(a|b)") && msgs[0].contains('3'),
        "the warning must name the short operand and the entry count: {msgs:?}",
    );
}

#[test]
fn unresolved_ref_reported() {
    let input = "glyph foo\nref nonexistent 0 0\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("unresolved ref")),
        "expected unresolved ref error, got: {issues:?}",
    );
}

/// Every `map BASE SELECTOR` check, driven through the same entry point the
/// build and the editor use. The shared prelude gives the pair something
/// valid to point at so that only the rule under test can fail.
fn uvs_issues(body: &str) -> Vec<Issue> {
    let input = format!("glyph zero 2 2\n@@@@\n@@@@\nglyph zero-emoji 2 2\n@@@@\n@@@@\n{body}");
    let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
}

fn has_error(issues: &[Issue], needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.severity == Severity::Error && i.message.contains(needle))
}

#[test]
fn a_valid_variation_sequence_is_clean() {
    let issues = uvs_issues("map U+0030 = zero\nmap U+0030 U+FE0F = zero-emoji\n");
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "expected no errors, got: {issues:?}",
    );
}

/// The two halves are not interchangeable, and a swapped line would build a
/// sequence no shaper could ever match — so both directions are errors.
#[test]
fn each_half_of_a_variation_sequence_must_be_the_right_kind() {
    let issues = uvs_issues("map U+0030 = zero\nmap U+0030 U+0031 = zero-emoji\n");
    assert!(
        has_error(&issues, "is not a variation selector"),
        "expected a selector-half error, got: {issues:?}",
    );

    let issues = uvs_issues("map U+FE0F U+FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "is a variation selector"),
        "expected a base-half error, got: {issues:?}",
    );
}

#[test]
fn only_one_half_of_a_variation_sequence_may_vary() {
    let issues = uvs_issues("map U+0030..0039 U+FE0E..FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "only one half"),
        "expected a both-vary error, got: {issues:?}",
    );
}

/// The fallback GSUB rule needs the base's own glyph as its first element,
/// so a pair whose base is unmapped would silently never fire.
#[test]
fn the_base_of_a_variation_sequence_must_be_mapped() {
    let issues = uvs_issues("map U+0030 U+FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "U+0030"),
        "expected an unmapped-base error, got: {issues:?}",
    );

    // ...and a base mapped only in *another* slice does not count, because
    // the pair and the base have to meet in the same face.
    let issues = uvs_issues(
        "slice wide\nslice narrow\nmap wide : U+0030 = zero\n\
             map narrow : U+0030 U+FE0F = zero-emoji\n",
    );
    assert!(
        has_error(&issues, "U+0030"),
        "expected a per-slice unmapped-base error, got: {issues:?}",
    );
}

/// Pasting `0️⃣` gives three characters, which cmap format 14 cannot hold.
/// The message has to say where the rest of the sequence goes, or the only
/// signal is a mapping that quietly never happens.
#[test]
fn a_pasted_longer_sequence_says_how_to_split_it() {
    let issues = uvs_issues("map 0\u{FE0F}\u{20E3} = zero-emoji\n");
    assert!(
        has_error(&issues, "remap"),
        "expected a split-it error naming remap, got: {issues:?}",
    );
}

#[test]
fn map_generate_rejects_a_variation_sequence() {
    let issues = uvs_issues("map U+0030 = zero\nmap generate U+0030 U+FE0F\n");
    assert!(
        has_error(&issues, "single character"),
        "expected a generate-sequence error, got: {issues:?}",
    );
}

/// A variation selector reaches the font only through a sequence. Mapping
/// one on its own would hand a source the glyph the fallback lookup owns,
/// and the two would then disagree about what that glyph is for.
#[test]
fn mapping_a_variation_selector_on_its_own_is_rejected() {
    let issues = uvs_issues("map U+FE0F = zero-emoji\n");
    assert!(
        has_error(&issues, "variation selector"),
        "expected a lone-selector error, got: {issues:?}",
    );
}

/// cmap format 14 is keyed by codepoint; the fallback lookup is keyed by
/// glyph. Where two characters share a base glyph the two halves of one
/// declaration stop agreeing, and the source has to be told.
#[test]
fn two_pairs_colliding_on_one_base_glyph_are_an_error() {
    let issues = uvs_issues(
        "glyph other 2 2\n@@@@\n@@@@\n\
             map U+0030 = zero\nmap U+0031 = zero\n\
             map U+0030 U+FE0F = zero-emoji\nmap U+0031 U+FE0F = other\n",
    );
    assert!(
        has_error(&issues, "keyed by glyph"),
        "expected a collision error, got: {issues:?}",
    );
}

#[test]
fn a_pair_on_a_shared_base_glyph_warns_about_over_firing() {
    let issues =
        uvs_issues("map U+0030 = zero\nmap U+0031 = zero\nmap U+0030 U+FE0F = zero-emoji\n");
    assert!(
        issues.iter().any(|i| i.severity == Severity::Warning
            && i.message.contains("U+0031")
            && i.message.contains("fallback lookup")),
        "expected an over-firing warning, got: {issues:?}",
    );
}

/// A base glyph only one character reaches is the ordinary case and has to
/// stay quiet, or the warning above would fire on every well-formed pair.
#[test]
fn a_pair_on_an_unshared_base_glyph_is_quiet() {
    let issues = uvs_issues("map U+0030 = zero\nmap U+0030 U+FE0F = zero-emoji\n");
    assert!(
        issues.is_empty(),
        "expected no issues at all, got: {issues:?}",
    );
}

/// The build names its synthesized selector glyphs `@vs-XXXX`, and that is
/// safe without a reserved-name rule because a source cannot produce the
/// name: `@` expands against the enclosing base into something else, and
/// with no base to expand against the name is invalid outright. This pins
/// the argument, since the safety of the whole scheme rests on it.
#[test]
fn a_source_cannot_write_the_synthesized_selector_name() {
    // With a preceding glyph, `@` expands and the name becomes another one.
    let doc = document_io::parse_document_from_str(
        "glyph base 2 2\n@@@@\n@@@@\nglyph @vs-FE0F 2 2\n@@@@\n@@@@\n",
        "test.unf".into(),
    )
    .unwrap();
    assert!(
        doc.items.iter().all(|item| !matches!(
            item,
            DocumentItem::Glyph { name: GlyphName(n), .. } if n == "@vs-FE0F"
        )),
        "the `@` should have expanded away, got {:?}",
        doc.items,
    );

    // With nothing to expand against — so, only as the very first glyph of
    // a project — it stays literal, and then it is not a valid name.
    let doc =
        document_io::parse_document_from_str("glyph @vs-FE0F 2 2\n@@@@\n@@@@\n", "test.unf".into())
            .unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error),
        "expected an invalid-name error, got: {issues:?}",
    );
}

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

/// A `-` anchor that name-matches a published `+` but size-mismatches it
/// is a near-miss (usually the wrong `:narrow`/`:wide` variant). It is an
/// error, not a note on the side: the mark attached to nothing, so the
/// composite is dropped rather than shipped with the mark at the pen.
#[test]
fn size_mismatched_attachment_reported() {
    let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1..2 0
glyph mark 2 1 mark
@@@@
anchor -above 0 0
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
        !issues.iter().any(|i| i.message.contains("only by name")),
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
fn duplicate_glyph_reported() {
    let input = "glyph foo 2 1\n..@@\nglyph foo 2 1\n@@..\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.message.contains("duplicate glyph")),
        "expected duplicate glyph warning, got: {issues:?}",
    );
}

#[test]
fn undefined_map_target_reported() {
    let input = "map A = nonexistent\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("nonexistent")),
        "expected undefined map target error, got: {issues:?}",
    );
}
#[test]
fn valid_document_has_no_issues() {
    let input = "\
glyph foo 2 1
..@@
map A = foo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(issues.is_empty(), "expected no issues, got: {issues:?}",);
}

// `testdata/` declares a single consistent `meta` because it has to
// stay a coherent project, so the broken variants are covered here.

#[test]
fn a_map_to_a_contentless_glyph_is_an_error() {
    // Neither a pixel grid nor a ref means the glyph never enters the
    // resolution cache, so it silently vanishes from the cmap. `advance`
    // does not make it buildable, but it does suppress the "has no
    // content" warning, so this used to pass without a single word.
    let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
map B = blank
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("'blank'")
            && i.message.contains("not built")),
        "mapping a contentless glyph must be an error, got: {issues:?}",
    );
}

#[test]
fn a_ref_and_a_remap_to_a_contentless_glyph_are_errors() {
    let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
glyph host
ref blank
map A = vis
remap liga : vis -> blank
feature liga for DFLT : liga
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    let errors: Vec<_> = issues
        .iter()
        .filter(|i| i.severity == Severity::Error && i.message.contains("'blank'"))
        .collect();
    assert!(
        errors.len() >= 2,
        "both the ref and the remap must be reported, got: {issues:?}",
    );
}

/// A glyph that is contentless but never used stays a warning — it builds
/// nothing, but it also breaks nothing.
#[test]
fn an_unused_contentless_glyph_is_not_an_error() {
    let input = "\
glyph pix 1 1
@@
glyph vis = pix
glyph blank advance 0
map A = vis
assume unused blank
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "an unused contentless glyph must not be an error, got: {issues:?}",
    );
}

// ------------------------------------------------------------------
// Glyph aliases (`glyph NAME = TARGET`); see `crate::alias`.
// ------------------------------------------------------------------

fn issues_for(input: &str) -> Vec<Issue> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
}

fn has(issues: &[Issue], severity: Severity, needle: &str) -> bool {
    issues
        .iter()
        .any(|i| i.severity == severity && i.message.contains(needle))
}

/// A `fill` naming a color that no `color` line declares silently fell
/// back to `fg` in the build; it has to be reported here instead.
#[test]
fn a_fill_naming_an_undeclared_color_is_a_warning() {
    let issues = issues_for("glyph a 1 1\n@@\n\nglyph b\nref a fill missing\n\nmap A = b\n");
    assert!(
        has(&issues, Severity::Warning, "undeclared color `missing`"),
        "{issues:?}"
    );
}

/// `color` aliases resolve in document order (see
/// `render::ttf_builder::color::collect_color_aliases`), so a value naming
/// a color declared later never resolves — silently, before this check.
#[test]
fn a_color_alias_used_before_its_declaration_is_a_warning() {
    let issues = issues_for("color x = y\ncolor y = #ff0000\n");
    assert!(has(&issues, Severity::Warning, "color `x`"), "{issues:?}");
}

#[test]
fn declared_color_uses_are_quiet() {
    let issues = issues_for(
        "color red = #ff0000\ncolor also-red = red\n\nglyph a 1 1\n@@\n\n\
             glyph b\nref a fill also-red\n\nmap A = b\n",
    );
    assert!(
        !issues.iter().any(|i| i.message.contains("color")),
        "{issues:?}"
    );
}

#[test]
fn an_alias_to_an_undefined_glyph_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = nope
map A = pix
map B = a
",
    );
    assert!(
        has(&issues, Severity::Error, "names undefined glyph `nope`"),
        "{issues:?}",
    );
}

/// An alias is a second name for a glyph, so a name that is both is two
/// answers to one question — and the expansion would silently keep the
/// glyph and drop the alias.
#[test]
fn a_name_that_is_both_a_glyph_and_an_alias_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a 1 1
@@
glyph a = pix
map A = a
",
    );
    assert!(
        has(&issues, Severity::Error, "both a glyph and an alias"),
        "{issues:?}",
    );
}

#[test]
fn an_alias_cycle_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = b
glyph b = a
map A = pix
",
    );
    assert!(has(&issues, Severity::Error, "is in a cycle"), "{issues:?}");
}

#[test]
fn a_duplicate_alias_is_an_error() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph other 1 1
..
glyph a = pix
glyph a = other
map A = a
",
    );
    assert!(
        has(&issues, Severity::Error, "declared more than once"),
        "{issues:?}",
    );
}

/// An alias nothing names is dead source, reported like an unused glyph —
/// but named as what it is, since the fix is to delete a line rather than
/// to find a home for a drawing.
#[test]
fn an_unused_alias_is_a_warning() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = pix
map A = pix
",
    );
    assert!(
        has(&issues, Severity::Warning, "glyph alias 'a' is unused"),
        "{issues:?}",
    );
}

/// The alias is a node of the reachability walk: naming it must keep both
/// it and its target alive.
#[test]
fn a_used_alias_keeps_its_target_used() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph a = pix
map A = a
",
    );
    assert!(
        !issues.iter().any(|i| i.message.contains("unused")),
        "neither the alias nor its target is unused, got: {issues:?}",
    );
}

/// An alias standing in for one half of a color/mono pair is used by the
/// name that pair synthesizes, exactly as a written-out `x:color` would be:
/// alternatives of a root name are roots, and an alias is one of them.
#[test]
fn an_alias_used_as_a_color_mono_half_is_not_unused() {
    let issues = issues_for(
        "\
glyph pix 1 1
@@
glyph y:mono
ref pix
glyph y:color
ref pix fill #ff0000
glyph x:mono
ref pix
glyph x:color = y:color
map X = x
map Y = y
",
    );
    assert!(
        !issues.iter().any(|i| i.message.contains("unused")),
        "the aliased color half is used by the synthesized `x`, got: {issues:?}",
    );
}

/// A pattern glyph declares one glyph per expanded name, whatever the block
/// holds — the expansions share the block's body, its pixel grid included,
/// exactly as they share its `ref` lines. So a block that states only a box is
/// the pattern form of `glyph blank 3 4`, and each expansion is that blank
/// glyph.
#[test]
fn a_pattern_glyph_stating_only_a_box_declares_each_expansion() {
    let input = "\
name-parts $ab = a b

glyph pat-($ab) 3 4
map A|B = pat-($ab)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.severity == Severity::Error),
        "a boxed pattern glyph declares `pat-a`/`pat-b` like any other glyph, got: {issues:?}",
    );
}

/// A block that declares *nothing* is still an error — but by the ordinary
/// contentless-glyph rule, reported per expanded name rather than by a rule of
/// its own about patterns.
#[test]
fn an_empty_pattern_glyph_is_an_error() {
    for body in ["", " advance 0"] {
        let input = format!(
            "\
name-parts $ab = a b

glyph pix 1 1
@@
glyph pat-($ab){body}
map A|B = pat-($ab)
"
        );
        let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error
                && i.message.contains("'pat-a'")
                && i.message.contains("not built")),
            "an empty pattern glyph must be an error (body {body:?}), got: {issues:?}",
        );
    }
}

/// A `name-parts` value is a pattern, so the declaration itself can be
/// over the expansion limit — before any glyph line refers to it.
#[test]
fn an_oversized_name_parts_binding_is_an_error() {
    let input = format!(
        "name-parts $many = x($1..{})\n",
        crate::pattern::MAX_EXPANSION + 1
    );
    let doc = document_io::parse_document_from_str(&input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("name part `$many`")),
        "an oversized binding must be an error, got: {issues:?}",
    );
}

#[test]
fn many_to_many_remap_is_an_error() {
    // Neither a ligature nor a multiple substitution can express this, and
    // guessing one of them silently loses half the rule.
    let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
glyph d 1 1
@@
map A = a
map B = b
remap liga : a b -> c d
feature liga for DFLT : liga
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("no OpenType lookup type")),
            "a 2-to-2 remap must be an error, got: {issues:?}",
        );
}

#[test]
fn many_to_nothing_remap_is_an_error() {
    let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
map A = a
map B = b
remap liga : a b ->
feature liga for DFLT : liga
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("no OpenType lookup type")),
            "deleting a multi-glyph sequence must be an error, got: {issues:?}",
        );
}

#[test]
fn expressible_remap_shapes_are_quiet() {
    // one-to-one, one-to-many, one-to-nothing and many-to-one all have a
    // lookup type, so none of them may be reported.
    let input = "\
glyph a 1 1
@@
glyph b 1 1
@@
glyph c 1 1
@@
map A = a
map B = b
map C = c
remap g1 : a -> b
remap g2 : a -> b c
remap g3 : a ->
remap g4 : a b -> c
feature liga for DFLT : g1
feature liga for DFLT : g2
feature liga for DFLT : g3
feature liga for DFLT : g4
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("no OpenType lookup type")),
        "expressible remaps must be quiet, got: {issues:?}",
    );
}

#[test]
fn remap_pattern_operand_expansions_are_checked() {
    // Remap operands keep their patterns until the GSUB builder expands
    // them, and that builder drops rules whose glyphs have no id without
    // a word. Validation therefore has to expand them the same way.
    let input = "\
name-parts $ab = a b

glyph ok 2 1
@@..
map A = ok

remap liga : ok -> missing-($ab)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.message.contains("missing-a")),
        "expected the expanded remap target to be reported, got: {issues:?}",
    );
    assert!(
        issues.iter().any(|i| i.message.contains("missing-b")),
        "every expansion should be reported, got: {issues:?}",
    );
}

#[test]
fn remap_pattern_operand_that_resolves_is_quiet() {
    let input = "\
name-parts $ab = a b

glyph ok-a 2 1
@@..
glyph ok-b 2 1
.@@.
glyph present-a 2 1
@@..
glyph present-b 2 1
..@@
map A = ok-a
map B = ok-b

remap liga : ok-($ab) -> present-($ab)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("remap")),
        "a remap whose expansions all exist must be quiet, got: {issues:?}",
    );
}

#[test]
fn meta_ascent_plus_descent_must_equal_height() {
    let input = "meta height 16\nmeta ascent 12\nmeta descent 3\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("!= height")),
        "expected meta metric mismatch warning, got: {issues:?}",
    );
}

#[test]
fn meta_zero_height_reported() {
    let input = "meta height 0\nmeta ascent 0\nmeta descent 0\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("meta height is 0")),
        "expected zero-height error, got: {issues:?}",
    );
}

/// An unknown key is the whole reason `meta` exists as a checked directive:
/// the value it carries is invisible in the built font, so a typo that is
/// merely ignored is a typo that ships.
#[test]
fn meta_unknown_key_is_error() {
    let input = "meta famliy 16\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("famliy")),
        "expected unknown-key error, got: {issues:?}",
    );
}

/// Every kind of conflict is an error, and two `meta` lines setting the
/// same key are a conflict even when they agree.
#[test]
fn meta_duplicate_key_is_error() {
    let input = "meta height 16\nmeta height 16\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("height")
            && i.message.contains("more than once")),
        "expected duplicate-key error, got: {issues:?}",
    );
}

#[test]
fn meta_wrong_arity_is_error() {
    for input in ["meta height\n", "meta height 16 12\n"] {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            issues.iter().any(|i| i.severity == Severity::Error),
            "expected an arity error for {input:?}, got: {issues:?}",
        );
    }
}

#[test]
fn meta_non_numeric_metric_is_error() {
    let input = "meta height sixteen\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error),
        "expected a non-numeric error, got: {issues:?}",
    );
}

/// An `audit` rule is single-assignment like a `meta` key, and unreadable
/// lines are errors rather than rules that quietly stop checking.
#[test]
fn audit_lines_are_checked_and_assigned_once() {
    let errors = |input: &str| -> Vec<String> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.severity == Severity::Error)
            .map(|i| i.message)
            .collect()
    };
    assert!(errors("audit ideal-clearance han-* 0 1\n").is_empty());
    assert!(
        errors("audit clarence han-* 0 1\n")
            .iter()
            .any(|m| m.contains("unknown `audit` key")),
    );
    assert!(
        errors("audit ideal-clearance han-* 0\n")
            .iter()
            .any(|m| m.contains("takes a glyph-name prefix")),
    );
    assert!(
        errors("audit ideal-clearance han-* 0 1\naudit ideal-clearance han-* 0 2\n")
            .iter()
            .any(|m| m.contains("is set more than once")),
    );
    // A different prefix is a different rule, not a second answer.
    assert!(
        errors("audit ideal-clearance han-* 0 1\naudit ideal-clearance hang-* 0 2\n").is_empty(),
    );
}

/// A component that is a composite draws no pixels of its own, but it does
/// draw: it is flattened before it is measured, so a radical written as a `ref`
/// to a shared drawing is checked like any other part. Before it was, a line
/// through one was silently not measured at all — no warning, right or wrong.
#[test]
fn a_component_that_is_a_composite_is_measured() {
    let source = |right: &str| {
        format!(
            "\
audit ideal-clearance test-* 0 1

glyph l:4x4 4 4
@@@@....
@@@@....
@@@@....
@@@@....

glyph r:4x4 4 4
..@@@@@@
..@@@@@@
..@@@@@@
..@@@@@@

glyph through-ref:4x4 4 4
ref r:4x4

glyph test-x 8 4
⿰ l:4x4 1 {right}:4x4
"
        )
    };
    let clearances = |input: &str| -> Vec<String> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.message.contains("ideal"))
            .map(|i| i.message)
            .collect()
    };
    // The two parts leave two cells between them and none at the right edge.
    let drawn = clearances(&source("r"));
    assert!(!drawn.is_empty(), "the drawn part is measured");
    // The name is the only difference; the ink behind it is the same ink.
    let through_ref = clearances(&source("through-ref"));
    assert_eq!(
        through_ref
            .iter()
            .map(|m| m.replace("through-ref", "r"))
            .collect::<Vec<_>>(),
        drawn,
    );
}

/// A line the grammar cannot read is an error, and the pixel row that
/// misses its glyph's width is why. It parses as a directive like any
/// other unreadable line, so the row is dropped and the glyph builds from
/// whatever rows did fit — a blank or half-drawn glyph that a `map` then
/// maps a character to, with nothing but a warning to say so.
#[test]
fn a_pixel_row_that_does_not_fit_is_an_error() {
    let input = "\
glyph wide 4 2
@@@@
@@@@
map A = wide
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("unrecognized directive")
            && i.message.contains("@@@@")),
        "expected the short row to be an error, got: {issues:?}",
    );
}

/// `font-meta` became `meta`. A leftover line must not fall through to the
/// generic "unrecognized directive" report: it names the migration, so the
/// author is not left rereading a line that is spelled correctly for the
/// format it was written against.
#[test]
fn legacy_font_meta_is_error() {
    let input = "font-meta height 16 ascent 12 descent 4\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("font-meta")
            && i.message.contains("meta")),
        "expected a migration error, got: {issues:?}",
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

#[test]
fn unused_glyph_reported() {
    let input = "\
glyph used 2 1
..@@
map A = used

glyph orphan 2 1
@@..
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Warning
                && i.message.contains("glyph 'orphan' is unused")),
        "expected unused glyph warning, got: {issues:?}",
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'used' is unused")),
        "mapped glyph should not be reported as unused",
    );
}

#[test]
fn transitively_used_glyph_not_reported() {
    let input = "\
glyph base 2 1
..@@

glyph composite 2 1
@@..
ref base 0 0

map A = composite
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        "transitively used glyph should not be unused: {issues:?}",
    );
}

#[test]
fn mutually_referencing_cluster_reported() {
    let input = "\
glyph a 2 1
..@@
ref b 0 0

glyph b 2 1
@@..
ref a 0 0
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("glyph 'a' is unused")),
        "mutual ref cluster should be unused: {issues:?}",
    );
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("glyph 'b' is unused")),
        "mutual ref cluster should be unused: {issues:?}",
    );
}

#[test]
fn remap_target_counts_as_used() {
    let input = "\
glyph base 2 1
..@@
map A = base

glyph alt 2 1
@@..

remap liga : base -> alt
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'alt' is unused")),
        "remap target should count as used: {issues:?}",
    );
}

#[test]
fn alternative_glyph_used_when_base_used() {
    let input = "\
glyph stem 2 1
..@@
map A = stem

glyph stem:wide 2 1
@@..
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'stem:wide' is unused")),
        "alternative of used base should not be unused: {issues:?}",
    );
}

/// `keep` says the glyph is wanted whether or not anything reaches it, so
/// the unused warning — which exists to find glyphs nothing reaches — must
/// stay quiet for one, whether it has a body or not.
#[test]
fn kept_glyph_not_reported_unused() {
    for input in [
        "glyph held keep advance 0\n",
        "glyph held 2 1 keep\n@@..\n",
        "glyph held keep\nanchor +join 0 0\n",
    ] {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let issues = collect_issues(&[&doc]);
        assert!(
            !issues.iter().any(|i| i.message.contains("is unused")),
            "a kept glyph is never unused, for {input:?}: {issues:?}",
        );
    }
}

/// `.notdef` is kept without saying `keep`: it is the glyph a renderer
/// draws for an uncovered character, so nothing in the source names it and
/// the unused warning would fire on every font that draws one.
#[test]
fn notdef_not_reported_unused_without_keep() {
    let input = "glyph .notdef 2 1\n@@..\nglyph a 2 1\n..@@\nmap A = a\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        ".notdef is kept automatically: {issues:?}",
    );
}

#[test]
fn ref_to_bodiless_kept_glyph_not_reported_unbuilt() {
    // A dimension-less `glyph NAME keep` is a placeholder that *is*
    // built (an empty anchor-carrying entry, see `glyph_cache::seed_cache`)
    // and is exempt from the "has no content" warning above; the
    // expansion's "is not built" error must exempt it the same way.
    let input = "\
glyph held keep
anchor +join 0 0

glyph user 2 1
@@..
ref held
map A = user
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("not built")),
        "keep placeholder is built; a ref to it is fine: {issues:?}",
    );
}

#[test]
fn a_fourth_heading_level_is_an_error_and_the_three_are_not() {
    let input = "# one\n## two\n### three\n#### four\n";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    let heading: Vec<&Issue> = issues
        .iter()
        .filter(|i| i.message.contains("heading level"))
        .collect();
    assert_eq!(heading.len(), 1, "only `####` is reported: {issues:?}");
    assert_eq!(heading[0].severity, Severity::Error);
    assert!(heading[0].message.contains("level 4"), "{:?}", heading[0]);
}

#[test]
fn assert_same_distinct_not_unrecognized() {
    let input = "\
glyph a 2 1
..@@
glyph b 2 1
@@..
map A = a
map B = b

assert same a b
assert distinct a b
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("unrecognized directive")),
        "assert same/distinct should not be flagged as unrecognized: {issues:?}",
    );
}

#[test]
fn map_decomposed_without_decomposition_reported() {
    // 'A' is already in NFD, so `map A` cannot synthesize anything.
    let input = "\
glyph a 2 1
..@@
map U+0041 = a
map generate A
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error
                && i.message.contains("no canonical decomposition")),
        "expected no-decomposition error, got: {issues:?}",
    );
}

#[test]
fn map_decomposed_with_unmapped_component_reported() {
    // 'Ä' decomposes to U+0041 U+0308; U+0308 is not mapped.
    let input = "\
glyph a 2 1
..@@
map A = a
map generate Ä
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("unmapped codepoint")
            && i.message.contains("U+0308")),
        "expected unmapped component error, got: {issues:?}",
    );
}

#[test]
fn map_decomposed_fully_mapped_accepted() {
    let input = "\
glyph a 2 1
..@@
glyph dieresis 2 1
@@..
map A = a
map U+0308 = dieresis
map generate Ä
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(
            |i| i.message.contains("decomposition") || i.message.contains("unmapped codepoint")
        ),
        "fully mapped decomposition should be accepted, got: {issues:?}",
    );
}

#[test]
fn assume_unused_suppresses_warning() {
    let input = "\
glyph orphan 2 1
@@..

glyph other 2 1
..@@

assume unused orphan
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("glyph 'orphan' is unused")),
        "assume unused should suppress warning: {issues:?}",
    );
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("glyph 'other' is unused")),
        "non-assumed glyph should still be reported: {issues:?}",
    );
    assert!(
        !issues
            .iter()
            .any(|i| i.message.contains("unrecognized directive")),
        "assume unused should not be flagged as unrecognized: {issues:?}",
    );
}

fn group_issues(text: &str) -> Vec<Issue> {
    let doc = document_io::parse_document_from_str(text, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.message.contains("remap group"))
        .collect()
}

#[test]
fn remap_group_ordering_cycle_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap y : a -> a\n\
             remap group x after y\nremap group y after x\n",
    );
    assert_eq!(issues.len(), 2, "one per declaration, got: {issues:?}");
    assert!(
        issues
            .iter()
            .all(|i| i.severity == Severity::Error && i.message.contains("ordering cycle")),
        "got: {issues:?}",
    );
}

#[test]
fn remap_group_after_undefined_group_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap group x after nope\n",
    );
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("undefined group 'nope'")),
        "got: {issues:?}",
    );
}

#[test]
fn remap_group_declared_twice_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nmap A = a\n\
             remap x : a -> a\nremap group x\nremap group x\n",
    );
    assert!(
            issues
                .iter()
                .any(|i| i.severity == Severity::Error
                    && i.message.contains("declared more than once")),
            "got: {issues:?}",
        );
}

#[test]
fn remap_group_without_rules_reported() {
    let issues = group_issues("remap group lonely\n");
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Warning && i.message.contains("has no rules")),
        "got: {issues:?}",
    );
}

/// A `feature` may be written above every rule of the group it attaches;
/// the check used to depend on scan order and would call that undefined.
#[test]
fn feature_may_precede_the_rules_of_its_group() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nmap A = a\nmap B = b\n\
             feature ccmp for DFLT : late\nremap late : a -> b\n",
    );
    assert!(issues.is_empty(), "got: {issues:?}");
}

#[test]
fn reversed_group_with_a_non_single_rule_reported() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nglyph c = pix\n\
             map A = a\nmap B = b\nmap C = c\n\
             remap x : a -> b\nremap x : a b -> c\nremap group x reversed\n",
    );
    assert!(
        issues.iter().any(|i| i.severity == Severity::Error
            && i.message.contains("reversed")
            && i.message.contains("one glyph")),
        "got: {issues:?}",
    );
}

/// The same rule is perfectly fine in a group that is not reversed.
#[test]
fn a_ligature_is_only_rejected_when_the_group_is_reversed() {
    let issues = group_issues(
        "glyph pix 1 1\n@@\nglyph a = pix\nglyph b = pix\nglyph c = pix\n\
             map A = a\nmap B = b\nmap C = c\n\
             remap x : a b -> c\nremap group x\n",
    );
    assert!(issues.is_empty(), "got: {issues:?}");
}

/// A property value the UCD does not use is an error: the line exists to be
/// read, and a value nothing can be checked against is worse than silence.
#[test]
fn prop_property_values_are_checked_against_the_ucd_short_names() {
    let src = "prop U+E000 = `X` gc Xx eaw WW\nprop U+E001 gc So eaw W\n";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
    assert!(
        msgs.iter()
            .any(|m| m.contains("`gc Xx` is not a General_Category")),
        "{msgs:?}",
    );
    assert!(
        msgs.iter()
            .any(|m| m.contains("`eaw WW` is not an East_Asian_Width")),
        "{msgs:?}",
    );
    // The well-formed line draws no complaint of its own.
    assert!(!msgs.iter().any(|m| m.contains("U+E001")), "{msgs:?}");
}

/// A character spelling that covers nothing — a backwards range — would
/// otherwise be a line that quietly never applies to anything.
#[test]
fn a_prop_line_that_names_no_character_is_an_error() {
    let src = "prop U+E00F..E000 gc So\n";
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("names no character")),
        "{issues:?}",
    );
}

/// Messages of one severity, for the tests that name their own filter.
fn messages_matching(input: &str, needle: &str) -> Vec<String> {
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    collect_issues(&[&doc])
        .into_iter()
        .filter(|i| i.message.contains(needle))
        .map(|i| format!("{:?}: {}", i.severity, i.message))
        .collect()
}

/// A single-substitution lookup covers each glyph once, so a second rule for
/// a glyph the group already substitutes is dropped when the lookup is built.
/// It used to be dropped in silence — and two rules land on one glyph without
/// looking like it whenever a name reaches the same glyph twice: through an
/// alias, or through an implicit merge (`crate::merge`), which is the whole
/// reason a merge cannot break a font quietly.
#[test]
fn a_remap_rule_shadowed_by_an_earlier_one_warns() {
    let msgs = messages_matching(
        "\
glyph a 1 1
@@
glyph x 1 1
..
glyph y 1 1
@@
glyph a-alias = a
map A = a
map X = x
map Y = y
remap sub : a -> x
remap sub : a-alias -> y
feature ccmp for DFLT : sub
",
        "shadow",
    );
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    assert!(msgs[0].starts_with("Warning"), "{msgs:?}");
}

/// The same source with the same target is a duplicate, not a lost rule, and
/// a group that is not one single-substitution lookup keeps its rule order in
/// the lookup itself — neither may warn.
#[test]
fn an_identical_or_contextual_rule_is_not_shadowed() {
    let msgs = messages_matching(
        "\
glyph a 1 1
@@
glyph x 1 1
..
glyph y 1 1
@@
glyph a-alias = a
map A = a
map X = x
map Y = y
remap sub : a -> x
remap sub : a-alias -> x
remap ctx : a -> x
remap ctx : y | a-alias -> y
feature ccmp for DFLT : sub
feature ccmp for DFLT : ctx
",
        "shadow",
    );
    assert!(msgs.is_empty(), "{msgs:?}");
}

/// A component named through a `glyph A = B` alias: the name the check sees has
/// been canonicalized to the drawing's own (`-r`), but the slot the author
/// picked is the one the *written* name states (`-c`). Ranking the canonical
/// name warns that every aliased component sits in the wrong slot, which is the
/// one thing the alias was written to say is fine.
#[test]
fn an_aliased_component_is_ranked_on_the_name_as_written() {
    let source = |middle: &str| {
        format!(
            "\
glyph a:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r:4x4-r 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph r:4x4-c = r:4x4-r

glyph b:4x4 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@

glyph test-x 12 4
\u{2FF2} a:4x4 {middle} b:4x4
"
        )
    };
    let slots = |input: &str| -> Vec<String> {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        collect_issues(&[&doc])
            .into_iter()
            .filter(|i| i.message.contains("sits in the"))
            .map(|i| i.message)
            .collect()
    };
    // The alias names the middle slot, which is the slot it sits in.
    assert!(
        slots(&source("r:4x4-c")).is_empty(),
        "{:?}",
        slots(&source("r:4x4-c"))
    );
    // The drawing's own name still says `-r`, and that one does warn.
    assert_eq!(slots(&source("r:4x4-r")).len(), 1);
}

/// A `$-N` back-reference on a `ref`, an IDC component or an alias target is a
/// use of what the group names, exactly as writing the group out again is. The
/// reachability walk reads the source rather than the expansion, so it has to
/// substitute the item's own captures itself or every glyph named that way
/// reads as unused.
#[test]
fn back_referenced_ref_target_is_a_use() {
    let input = "\
glyph part-a 2 1
..@@

glyph part-b 2 1
@@..

glyph outer-(a|b) 2 1
ref part-($-1) 0 0

map A|B = outer-(a|b)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        "a `$-N` ref target uses the glyphs its group names: {issues:?}",
    );
}

#[test]
fn back_referenced_alias_target_is_a_use() {
    let input = "\
glyph part-a 2 1
..@@

glyph part-b 2 1
@@..

glyph outer-(a|b) = part-($-1)

map A|B = outer-(a|b)
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        !issues.iter().any(|i| i.message.contains("is unused")),
        "a `$-N` alias target uses the glyphs its group names: {issues:?}",
    );
}

/// Validation used to expand for the *primary* face alone, so a line stated
/// for a slice only some other face includes was dropped before any diagnostic
/// about it could exist — the same line reported an error under the primary
/// slice and nothing at all under the other one. The expansion validation
/// reads is the union of every declared slice for exactly this reason.
#[test]
fn a_non_primary_slice_is_validated_too() {
    let input = "\
face main : sa
meta main : family Main
face other : sb
meta other : family Other
slice sa
slice sb

glyph aa 2 1
@@..

map sa : U+0041 = aa
map sb : U+0042 = nosuchglyph
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let issues = collect_issues(&[&doc]);
    assert!(
        issues
            .iter()
            .any(|i| i.severity == Severity::Error && i.message.contains("nosuchglyph")),
        "a bad `map` in a non-primary face's slice must still be reported: {issues:?}",
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
