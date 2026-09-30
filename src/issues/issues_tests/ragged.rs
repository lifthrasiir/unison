//! Lock-step group and remap operand raggedness warnings.

use super::*;

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
