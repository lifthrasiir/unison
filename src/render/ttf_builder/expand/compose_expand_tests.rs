//! The diagnostics an IDC line produces once its components have been looked
//! up — which is here rather than in `compose.rs`, because whether a component
//! name resolves is a question only the whole expansion can answer.

use crate::document_io::parse_document_from_str;
use crate::issues::Severity;

fn expand(src: &str) -> super::Expansion {
    let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
    let docs = vec![&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    super::expand_documents(&docs, &name_parts)
}

/// A cancelled expansion gives up and says so, including one cancelled
/// while its `map` alternatives are being settled on every core; an
/// uncancelled one is exactly [`super::expand_documents_for`].
#[test]
fn a_cancelled_expansion_returns_nothing() {
    let doc = parse_document_from_str(
        "glyph a 1 1\n@@\nglyph b-4e01 1 1\n@@\nmap U+4E00..4EFF = b-($#4e00..4eff) a\n",
        "test.unf".into(),
    )
    .unwrap();
    let docs = vec![&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let faces = crate::faces::FaceSet::collect(&docs);
    let items = |e: &super::Expansion| e.items.len();

    let cancel = crate::cancel::CancelToken::new();
    let uncancelled = super::expand_documents_cancellable(&docs, &name_parts, &faces, &cancel);
    assert_eq!(
        uncancelled.as_ref().map(items),
        Some(items(&super::expand_documents_for(
            &docs,
            &name_parts,
            &faces
        )))
    );

    cancel.cancel();
    assert!(super::expand_documents_cancellable(&docs, &name_parts, &faces, &cancel).is_none());
}

fn of(expansion: &super::Expansion, severity: Severity) -> Vec<&str> {
    expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == severity)
        .map(|d| d.message.as_str())
        .collect()
}

/// Every ref of `glyph`, with the offset the line derived for it.
fn placed<'a>(expansion: &'a super::Expansion, glyph: &str) -> Vec<(&'a str, (i16, i16))> {
    expansion
        .items()
        .filter_map(|item| match item {
            crate::document::DocumentItem::Glyph { name, body } if name.display() == glyph => Some(
                body.refs
                    .iter()
                    .map(|r| (r.name.as_str(), r.offset.unwrap_or_default())),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}
fn refs_of<'a>(expansion: &'a super::Expansion, glyph: &str) -> Vec<&'a str> {
    expansion
        .items()
        .filter_map(|item| match item {
            crate::document::DocumentItem::Glyph { name, body } if name.display() == glyph => {
                Some(body.refs.iter().map(|r| r.name.as_str()))
            }
            _ => None,
        })
        .flatten()
        .collect()
}

/// The state every Han glyph is populated in:
/// both components bare, and neither naming anything yet. The line is a
/// Todo per component and **nothing else** — in particular not an
/// `unresolved ref` for each derived ref, which would turn a work queue of
/// 20k items into 40k errors and fail every build and CI run over source
/// that is merely unfinished (see [`Severity`]).
///
/// The refs are still emitted and still unresolved: that is what keeps the
/// glyph out of the font, and it is a mechanism rather than a complaint.
#[test]
fn an_undecided_component_naming_nothing_is_a_todo_and_not_an_unresolved_ref() {
    let expansion = expand("glyph han-6cb3 4 2\n\u{2FF0} han-6c35 han-53ef\n");
    assert!(
        of(&expansion, Severity::Error).is_empty(),
        "{:?}",
        of(&expansion, Severity::Error)
    );
    assert_eq!(of(&expansion, Severity::Todo).len(), 2);
    assert_eq!(
        refs_of(&expansion, "han-6cb3"),
        vec!["han-6c35", "han-53ef"]
    );
}

/// Only the *undecided* half stands down. A component that picked a
/// variant made a claim, and a claim about a glyph that does not exist is
/// an ordinary error — the author decided, and decided wrong.
#[test]
fn a_decided_component_naming_nothing_is_still_an_error() {
    let expansion = expand("glyph han-6cb3 4 2\n\u{2FF0} han-6c35:2x2 han-53ef\n");
    let errors = of(&expansion, Severity::Error);
    assert!(
        errors.iter().any(|m| m.contains("'han-6c35:2x2'")),
        "{errors:?}"
    );
    assert!(
        !errors.iter().any(|m| m.contains("'han-53ef'")),
        "{errors:?}"
    );
}

/// The suppression is scoped to the name the IDC line left undecided, not
/// to the glyph: a hand-written `ref` on the same block that names nothing
/// is still reported.
#[test]
fn a_ref_beside_an_undecided_idc_line_is_still_reported() {
    let expansion = expand("glyph han-6cb3 4 2\n\u{2FF0} han-6c35 han-53ef\nref gone 0 0\n");
    assert_eq!(
        of(&expansion, Severity::Error),
        vec!["unresolved ref 'gone'"]
    );
}

/// A part drawn narrower than its box, seen end to end: the boxes tile the
/// parent perfectly and the ink still leaves a 2-cell canyon.
const CANYON: &str = "\
glyph p-a:2x2 2 2
@@..
@@..
glyph p-b:2x2 2 2
..@@
..@@
glyph p-x 4 2
\u{2FF0} p-a:2x2 p-b:2x2
";

#[test]
fn a_clearance_outside_the_ideal_range_is_a_chore() {
    let expansion = expand(&format!("audit ideal-clearance p-* 0 1\n{CANYON}"));
    assert!(of(&expansion, Severity::Error).is_empty());
    assert!(of(&expansion, Severity::Warning).is_empty());
    let warnings = of(&expansion, Severity::Chore);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    assert!(
        warnings[0].contains("leaves 2 between 'p-a:2x2' and 'p-b:2x2'"),
        "{warnings:?}",
    );
    assert!(warnings[1].contains("leaves 2 in total"), "{warnings:?}");
}

#[test]
fn a_glyph_no_rule_reaches_is_not_measured() {
    // The same source, held only by a prefix that does not name it.
    let expansion = expand(&format!("audit ideal-clearance q-* 0 1\n{CANYON}"));
    assert!(of(&expansion, Severity::Chore).is_empty());
    // …and with no rule at all.
    let expansion = expand(CANYON);
    assert!(of(&expansion, Severity::Chore).is_empty());
}

/// The parts of two glyphs written by one line, as a Han source writes a
/// family: the components expand in lock-step with the block's name, and
/// the *layout* is solved per glyph — the same line puts the second part at
/// 1 in one expansion and at 2 in the next, because that expansion's first
/// part is that much wider.
#[test]
fn a_pattern_block_expands_its_idc_line_per_glyph() {
    let expansion = expand(
        "\
glyph p-a1:1x2 1 2
@.
@.
glyph p-a2:2x2 2 2
@@
@@
glyph p-b1:3x2 3 2
@@@
@@@
glyph p-b2:2x2 2 2
@@
@@
glyph p-x(1|2) 4 2
\u{2FF0} p-a(1|2):(1|2)x2 p-b(1|2):(3|2)x2
",
    );
    assert!(
        of(&expansion, Severity::Error).is_empty(),
        "{:?}",
        of(&expansion, Severity::Error)
    );
    assert_eq!(
        placed(&expansion, "p-x1"),
        vec![("p-a1:1x2", (0, 0)), ("p-b1:3x2", (1, 0))],
    );
    assert_eq!(
        placed(&expansion, "p-x2"),
        vec![("p-a2:2x2", (0, 0)), ("p-b2:2x2", (2, 0))],
    );
}

/// A finding about one expansion of a pattern line names the glyph it is
/// about, so the specimen faults that cell rather than every glyph the line
/// declares (see [`crate::glyph_flags`]).
#[test]
fn a_finding_about_one_expansion_names_that_glyph() {
    let expansion = expand(
        "\
glyph p-a:2x2 2 2
@@
@@
glyph p-x(1|2) 4 2
\u{2FF0} p-a:2x2 (p-a:2x2|p-gone:2x2)
",
    );
    let faulted: Vec<_> = expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| (d.glyph.as_deref(), d.message.as_str()))
        .collect();
    // The missing component and the ref it derived, both about `p-x2`
    // alone — `p-x1` is written by the same line and is faultless.
    assert_eq!(faulted.len(), 2, "{faulted:?}");
    assert!(
        faulted.iter().all(|(g, _)| *g == Some("p-x2")),
        "{faulted:?}"
    );
}
/// `$$` is a cell the source keeps clear on purpose, so it holds a
/// neighbour off exactly as ink does — that is what it is for.
#[test]
fn a_hardblank_is_a_frontier() {
    let expansion = expand(
        "audit ideal-clearance p-* 0 1
glyph p-a:2x2 2 2
@@..
@@..
glyph p-b:2x2 2 2
$$@@
$$@@
glyph p-x 4 2
\u{2FF0} p-a:2x2 p-b:2x2
",
    );
    // 0 at each edge and 1 down the middle, where the ink alone would have
    // read 2 and failed.
    assert!(of(&expansion, Severity::Chore).is_empty());
}
