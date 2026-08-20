use super::*;

fn caps(pattern: &str, name: &str) -> Option<Vec<String>> {
    ExistsPattern::parse(pattern).unwrap().capture(name)
}

#[test]
fn matches_the_whole_name_only() {
    let p = ExistsPattern::parse("han-([0-9a-f]{4,5}):15x16").unwrap();
    assert!(p.is_match("han-4e00:15x16"));
    assert!(p.is_match("han-20000:15x16"));
    // Anchored: a longer name containing the pattern is not a match.
    assert!(!p.is_match("xhan-4e00:15x16"));
    assert!(!p.is_match("han-4e00:15x16-alt"));
    // The size suffix is part of the pattern, so other variants stay out.
    assert!(!p.is_match("han-4e00:7x16"));
    // Three hex digits is under the {4,5} floor.
    assert!(!p.is_match("han-4e0:15x16"));
}

#[test]
fn capture_zero_is_the_whole_name() {
    assert_eq!(
        caps("han-([0-9a-f]{4,5}):15x16", "han-4e00:15x16"),
        Some(vec!["han-4e00:15x16".to_string(), "4e00".to_string()]),
    );
}

#[test]
fn several_captures_are_numbered_in_source_order() {
    let got = caps("han-([0-9a-f]{4,5}):([0-9]+)x16", "han-4e00:15x16").unwrap();
    assert_eq!(got, ["han-4e00:15x16", "4e00", "15"]);
}

#[test]
fn a_group_matching_no_alternative_is_an_empty_slot() {
    // `$2` took part in no alternative, but it still occupies slot 2 so that
    // `$1` cannot silently become what `$2` was written for.
    let p = ExistsPattern::parse("han-(a)-x|han-(b)-y").unwrap();
    assert_eq!(p.capture_count(), 2);
    assert_eq!(p.capture("han-a-x").unwrap(), ["han-a-x", "a", ""]);
    assert_eq!(p.capture("han-b-y").unwrap(), ["han-b-y", "", "b"]);
}

#[test]
fn a_non_capturing_group_takes_no_slot() {
    let p = ExistsPattern::parse("han-(?:x|y)-([0-9])").unwrap();
    assert_eq!(p.capture_count(), 1);
    assert_eq!(p.capture("han-x-3").unwrap(), ["han-x-3", "3"]);
}

#[test]
fn no_match_is_none() {
    assert_eq!(caps("han-([0-9])", "kana-4"), None);
}

/// A bare `.` matches `(`, `|` and `$`, so a match could carry name-pattern
/// syntax into a glyph name. It is rejected in favour of an explicit class.
#[test]
fn a_bare_dot_is_rejected() {
    let err = ExistsPattern::parse("han-(.)").unwrap_err();
    assert!(err.contains("character class"), "{err}");
    // The escaped form is a literal dot, which *is* a name character.
    let p = ExistsPattern::parse(r"han-(x)\.alt").unwrap();
    assert!(p.is_match("han-x.alt"));
}

#[test]
fn classes_reaching_outside_name_characters_are_rejected() {
    for pattern in [r"han-(\w+)", "han-([^x])", "han-([a-~])", "han-([ -/])"] {
        assert!(
            ExistsPattern::parse(pattern).is_err(),
            "{pattern} should be rejected"
        );
    }
    for pattern in ["han-([0-9a-f]+)", "han-([-_.:])", "han-([A-Za-z0-9]{2})"] {
        assert!(
            ExistsPattern::parse(pattern).is_ok(),
            "{pattern} should be accepted"
        );
    }
}

#[test]
fn anchors_are_rejected() {
    for pattern in ["^han-(x)", "han-(x)$", r"\bhan-(x)", r"\Ahan-(x)\z"] {
        let err = ExistsPattern::parse(pattern).unwrap_err();
        assert!(err.contains("not allowed here"), "{pattern}: {err}");
    }
}

#[test]
fn too_many_captures_is_an_error() {
    let ten = "(a)".repeat(10);
    let err = ExistsPattern::parse(&ten).unwrap_err();
    assert!(err.contains("max 9"), "{err}");
    let nine = "(a)".repeat(9);
    assert_eq!(ExistsPattern::parse(&nine).unwrap().capture_count(), 9);
}

#[test]
fn an_unparsable_pattern_reports_one_line() {
    let err = ExistsPattern::parse("han-([0-9").unwrap_err();
    assert!(!err.contains('\n'), "{err}");
    assert!(err.contains("han-([0-9"), "{err}");
}

#[test]
fn an_empty_pattern_is_an_error() {
    assert!(ExistsPattern::parse("").is_err());
}

// --- The directive end to end -------------------------------------------
//
// Built through `Resolution::compute`, which is the pipeline the build, the
// editor and the validation pass all share, so what these pin is what a font
// gets rather than what one stage does.

use crate::document::{Document, DocumentItem};
use crate::document_io::parse_document_from_str;
use crate::resolve::Resolution;

fn doc(src: &str) -> Document {
    parse_document_from_str(src, "t.unf".into()).unwrap()
}

/// Every glyph name the expansion declares, sorted.
fn declared(resolution: &Resolution) -> Vec<String> {
    let mut names: Vec<String> = resolution
        .expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Glyph { name, .. } => Some(name.0.clone()),
            _ => None,
        })
        .collect();
    names.sort();
    names
}

/// `codepoint => glyph`, sorted, for every `map` the expansion produced —
/// through `expand_map_pairs`, since a `map` target is a pattern that stays
/// unexpanded on the item until the cmap is collected.
fn maps(resolution: &Resolution) -> Vec<String> {
    let mut out: Vec<String> = resolution
        .expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Map {
                char_repr,
                selector,
                glyph,
                ..
            } => {
                let pairs = crate::render::ttf_builder::expand_map_pairs(char_repr, glyph);
                Some(format!(
                    "{char_repr}{} => {}",
                    selector
                        .as_deref()
                        .map(|s| format!(" {s}"))
                        .unwrap_or_default(),
                    pairs
                        .iter()
                        .map(|(_, n)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ))
            }
            _ => None,
        })
        .collect();
    out.sort();
    out
}

fn errors(resolution: &Resolution) -> Vec<String> {
    resolution
        .expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::issues::Severity::Error)
        .map(|d| d.message.clone())
        .collect()
}

const PARTS: &str = "\
glyph han-4e00:15x16 1 1
@@
glyph han-4e01:15x16 1 1
@@
glyph han-4e01:7x16 1 1
@@
";

/// The case the directive was added for: three `han-XXXX:15x16` drawn, so three
/// `han-XXXX` built — and the `7x16` variant, which the pattern does not
/// describe, builds nothing.
#[test]
fn a_search_declares_one_glyph_per_match() {
    let d = doc(&format!(
        "{PARTS}\
exists han-([0-9a-f]{{4,5}}):15x16
glyph han-($1) 16 16 advance 16
ref ($0)
"
    ));
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert_eq!(
        declared(&r),
        [
            "han-4e00",
            "han-4e00:15x16",
            "han-4e01",
            "han-4e01:15x16",
            "han-4e01:7x16",
        ]
    );
}

/// `$0` is the whole matched name, which is what the scoped block refers to.
#[test]
fn the_scoped_block_refs_what_the_search_found() {
    let d = doc(&format!(
        "{PARTS}\
exists han-([0-9a-f]{{4,5}}):15x16
glyph han-($1) 16 16 advance 16
ref ($0)
"
    ));
    let r = Resolution::compute(&[&d]);
    let refs: Vec<(String, Vec<String>)> = r
        .expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Glyph { name, body } if !name.0.contains(':') => Some((
                name.0.clone(),
                body.refs.iter().map(|x| x.name.clone()).collect(),
            )),
            _ => None,
        })
        .collect();
    assert_eq!(
        refs,
        [
            ("han-4e00".to_string(), vec!["han-4e00:15x16".to_string()]),
            ("han-4e01".to_string(), vec!["han-4e01:15x16".to_string()]),
        ]
    );
}

/// The codepoint is computed from the capture rather than written, which is the
/// half `ifexists` had no answer for: today's `map U+4E00..9FFF = han-($#…)
/// ifexists` claims the whole range and lets resolution drop what is missing.
#[test]
fn a_scoped_map_computes_its_codepoint() {
    let d = doc(&format!(
        "{PARTS}\
exists han-([0-9a-f]{{4,5}}):15x16
glyph han-($1) 16 16 advance 16
ref ($0)
exists han-([0-9a-f]{{4,5}}):15x16
map U+($1) = han-($1)
"
    ));
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert_eq!(maps(&r), ["U+4E00 => han-4e00", "U+4E01 => han-4e01"]);
}

/// `U+BASE+($N)` for the offset form, and both halves of a variation sequence
/// take the same spelling.
#[test]
fn a_scoped_map_offsets_and_writes_a_variation_sequence() {
    let d = doc("\
glyph han-4e00.0:15x16 1 1
@@
exists han-([0-9a-f]{4,5})\\.([0-9a-f]):15x16
glyph han-($1).($2) 16 16 advance 16
ref ($0)
exists han-([0-9a-f]{4,5})\\.([0-9a-f]):15x16
map U+($1) U+E0100+($2) = han-($1).($2)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert_eq!(maps(&r), ["U+4E00 U+E0100 => han-4e00.0"]);
}

/// A search that matches a name another search declared: the round after the
/// first picks it up.
#[test]
fn one_search_may_find_what_another_declared() {
    let d = doc("\
glyph base-01 1 1
@@
exists base-([0-9]{2})
glyph mid-($1) 1 1
ref ($0)
exists mid-([0-9]{2})
glyph top-($1) 1 1
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert_eq!(declared(&r), ["base-01", "mid-01", "top-01"]);
}

/// Two searches feeding each other have no least fixpoint, so the round budget
/// runs out and the build fails rather than settling on a round count.
#[test]
fn searches_that_feed_each_other_are_an_error() {
    let d = doc("\
glyph a-x 1 1
@@
exists a-([a-z]+)
glyph b-($1)x 1 1
ref ($0)
exists b-([a-z]+)
glyph a-($1)x 1 1
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    let errs = errors(&r);
    assert!(
        errs.iter().any(|e| e.contains("cycle")),
        "expected a cycle error, got {errs:?}"
    );
}

/// On-demand names are an infinite set, so a search cannot enumerate them and
/// must not answer for them — unlike `ifexists`, which does.
#[test]
fn on_demand_names_are_not_found() {
    let d = doc("\
exists ([0-9]+x[0-9]+)
glyph box-($1) 4 4
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(declared(&r), Vec::<String>::new());
    let warnings: Vec<&String> = r
        .expansion
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::issues::Severity::Warning)
        .map(|d| &d.message)
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("matches no declared")),
        "{warnings:?}"
    );
}

/// An alias is a name a `ref` may use, so a search finds it — which is how
/// `glyph han-4ee4:15x16 = han-4ee4-k:15x16` gets a base glyph built for it.
#[test]
fn an_alias_is_found_like_any_other_name() {
    let d = doc("\
glyph part-a-k 1 1
@@
glyph part-a = part-a-k
exists part-([a-z])
glyph made-($1) 1 1
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert!(
        declared(&r).contains(&"made-a".to_string()),
        "{:?}",
        declared(&r)
    );
}

/// What is refused is narrower: two matched names that are one glyph. `$0`
/// would have two values for it, and the block below would build it twice.
#[test]
fn finding_one_glyph_under_two_names_is_an_error() {
    let d = doc("\
glyph part-a 1 1
@@
glyph part-b = part-a
exists part-([a-z])
glyph made-($1) 1 1
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    let errs = errors(&r);
    assert!(
        errs.iter()
            .any(|e| e.contains("two names for the glyph `part-a`")),
        "expected a collision error, got {errs:?}"
    );
    // And the block it scoped builds nothing rather than a glyph named `$1`.
    assert_eq!(declared(&r), ["part-a"]);
}

/// A search that cannot run leaves the line below it standing for nothing, so
/// the unbindable `$N` on it is not a second finding.
#[test]
fn a_failed_search_silences_the_line_it_scoped() {
    let d = doc("\
glyph part-a 1 1
@@
exists part-(
glyph made-($1) 1 1
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    let errs = errors(&r);
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("invalid exists pattern"), "{errs:?}");
    assert_eq!(declared(&r), ["part-a"]);
}

/// The scope is the next line and only the next line.
#[test]
fn a_blank_line_below_an_exists_is_an_error() {
    let d = doc("\
glyph part-a 1 1
@@
exists part-([a-z])

glyph made-($1) 1 1
");
    let r = Resolution::compute(&[&d]);
    let errs = errors(&r);
    assert!(
        errs.iter().any(|e| e.contains("blank line")),
        "expected a scope error, got {errs:?}"
    );
}

/// `exists` does not stack, so `$N` never has two patterns to have come from.
#[test]
fn a_second_exists_below_an_exists_is_an_error() {
    let d = doc("\
glyph part-a 1 1
@@
exists part-([a-z])
exists part-([a-z])
glyph made-($1) 1 1
");
    let r = Resolution::compute(&[&d]);
    let errs = errors(&r);
    assert!(
        errs.iter().any(|e| e.contains("another `exists`")),
        "expected a scope error, got {errs:?}"
    );
}
