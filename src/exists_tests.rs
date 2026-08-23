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

/// The codepoint is computed from the capture rather than written, so the line
/// claims exactly the characters that were drawn and no others.
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

/// The variation-sequence checks read the *source*, so they have to read a
/// scoped `map` through its matches like every other consumer — otherwise the
/// `U+($1)` still standing on the line reads as a codepoint nobody could write.
#[test]
fn a_scoped_variation_sequence_is_validated_per_match() {
    let d = doc("\
glyph han-4e00:15x16 1 1
@@
glyph han-4e00.0:15x16 1 1
@@
exists han-([0-9a-f]{4,5}):15x16
glyph han-($1) 16 16 advance 16
ref ($0)
exists han-([0-9a-f]{4,5}):15x16
map U+($1) = han-($1)
exists han-([0-9a-f]{4,5})\\.([0-9a-f]):15x16
glyph han-($1).($2) 16 16 advance 16
ref ($0)
exists han-([0-9a-f]{4,5})\\.([0-9a-f]):15x16
map U+($1) U+E0100+($2) = han-($1).($2)
");
    let errs: Vec<String> = crate::issues::collect_issues(&[&d])
        .into_iter()
        .filter(|i| i.severity == crate::issues::Severity::Error)
        .map(|i| i.message)
        .collect();
    assert_eq!(errs, Vec::<String>::new());
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
/// must not answer for them.
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

/// Two matched names that are one glyph are not a search's problem: the block
/// below still names its output by the captures, so it builds two glyphs that
/// happen to share a shape. That is what an alias is for.
#[test]
fn finding_one_glyph_under_two_names_is_fine() {
    let d = doc("\
glyph part-a 1 1
@@
glyph part-b = part-a
exists part-([a-z])
glyph made-($1) 1 1
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    // Both names are built. They come out as one glyph id, because the two
    // blocks the pattern expands to `ref` the same thing and `crate::merge`
    // folds those — which is the same answer the source gave by aliasing.
    assert_eq!(declared(&r), ["made-a", "part-a"]);
    assert_eq!(
        r.expansion.aliases.resolved_target("made-b"),
        Some("made-a")
    );
}

/// The hazard the search itself never had to police: a pattern whose captures
/// do not tell two matches apart makes the block below declare one name twice.
/// That is a duplicate declaration like any other and is reported as one, on
/// the name that collided rather than on the `exists`.
#[test]
fn a_pattern_that_cannot_tell_two_matches_apart_declares_twice() {
    let d = doc("\
glyph part-a 1 1
@@
glyph part-a.0 1 1
@@
exists part-(a)(\\.0)?
glyph made-($1) 1 1
ref ($0)
");
    let issues = crate::issues::collect_issues(&[&d]);
    assert!(
        issues
            .iter()
            .any(|i| i.message.contains("duplicate glyph 'made-a'")),
        "expected a duplicate-glyph finding, got {:?}",
        issues.iter().map(|i| &i.message).collect::<Vec<_>>()
    );
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

// --- Search and navigation ----------------------------------------------

#[test]
fn a_template_denotes_the_names_its_search_could_produce() {
    let p = "han-([0-9a-f]{4,5}):15x16";
    assert_eq!(template_denotes(p, "han-($1)", "han-4e00"), Some(true));
    assert_eq!(template_denotes(p, "han-($1)", "han-20000"), Some(true));
    // The variant the search reads, not the glyph the header declares.
    assert_eq!(
        template_denotes(p, "han-($1)", "han-4e00:15x16"),
        Some(false)
    );
    // Outside what the capture can hold.
    assert_eq!(template_denotes(p, "han-($1)", "han-zzzz"), Some(false));
    assert_eq!(template_denotes(p, "han-($1)", "kana-4e00"), Some(false));
    // The literal text around the slot is literal, `.` included.
    assert_eq!(
        template_denotes(p, "han-($1).alt", "han-4e00.alt"),
        Some(true)
    );
    assert_eq!(
        template_denotes(p, "han-($1).alt", "han-4e00xalt"),
        Some(false)
    );
}

#[test]
fn capture_zero_denotes_the_whole_matched_name() {
    let p = "han-([0-9a-f]{4,5}):15x16";
    assert_eq!(
        template_denotes(p, "copy-($0)", "copy-han-4e00:15x16"),
        Some(true)
    );
    assert_eq!(
        template_denotes(p, "copy-($0)", "copy-han-4e00"),
        Some(false)
    );
}

#[test]
fn slots_are_numbered_by_the_groups_the_author_counted() {
    let p = "([a-z]+)-([0-9]+)";
    assert_eq!(template_denotes(p, "x-($2)-($1)", "x-12-ab"), Some(true));
    assert_eq!(template_denotes(p, "x-($2)-($1)", "x-ab-12"), Some(false));
    // A slot the pattern has no group for is no test at all.
    assert_eq!(template_denotes(p, "x-($3)", "x-1"), None);
}

#[test]
fn a_pattern_that_is_not_a_pattern_denotes_nothing() {
    assert_eq!(template_denotes("han-(", "han-($1)", "han-4e00"), None);
    assert_eq!(template_denotes("han-(.)", "han-($1)", "han-4e00"), None);
}

#[test]
fn an_exists_line_is_recognized_by_its_text() {
    assert_eq!(
        pattern_on_line("exists han-([0-9a-f]{4,5}):15x16"),
        Some("han-([0-9a-f]{4,5}):15x16".to_string()),
    );
    assert_eq!(pattern_on_line("  exists a-(x)"), Some("a-(x)".to_string()));
    assert_eq!(pattern_on_line("glyph a 1 1"), None);
    assert_eq!(pattern_on_line("exists a b"), None);
    assert_eq!(pattern_on_line("existsx a"), None);
}

/// The carry steps onto each line before it is read, so a line that starts the
/// next item is already ungoverned when its own names are looked at.
#[test]
fn the_carry_covers_the_block_and_stops_at_the_next_item() {
    let lines = [
        "glyph other 8 16",
        "exists han-(x)",
        "glyph han-($1) 16 16",
        "ref ($0) 1 0",
        "anchor top 0 0",
        "glyph next-($1) 8 16",
    ];
    let mut carry = Carry::default();
    let seen: Vec<Option<String>> = lines
        .iter()
        .map(|l| {
            carry.enter(l);
            carry.pattern().map(str::to_string)
        })
        .collect();
    let p = || Some("han-(x)".to_string());
    assert_eq!(seen, [None, p(), p(), p(), p(), None]);
}

/// A `map` is one line, so the carry is over after it.
#[test]
fn the_carry_over_a_map_lasts_one_line() {
    let lines = ["exists han-(x)", "map U+($1) = han-($1)", "map U+0041 = a"];
    let mut carry = Carry::default();
    let seen: Vec<Option<String>> = lines
        .iter()
        .map(|l| {
            carry.enter(l);
            carry.pattern().map(str::to_string)
        })
        .collect();
    assert_eq!(
        seen,
        [
            Some("han-(x)".to_string()),
            Some("han-(x)".to_string()),
            None
        ]
    );
}

/// A blank line ends the block, which is the same rule that makes a blank line
/// below an `exists` an error rather than a wider reach.
#[test]
fn a_blank_line_ends_the_carry() {
    let mut carry = Carry::default();
    for line in ["exists han-(x)", "glyph han-($1) 8 16", ""] {
        carry.enter(line);
    }
    assert_eq!(carry, Carry::None);
}

/// A scoped item runs **once per match**, with each `$N` bound to one string.
/// The alternative — one run with every slot bound to the whole list — made a
/// slot combine with the other groups on the line, so `(x|y)` beside a `($1)`
/// of three matches wrote three names rather than six, and writing the six
/// needed a `**N` multiplier on the group that had nothing to do with the
/// search.
#[test]
fn a_scoped_block_expands_its_own_groups_per_match() {
    let d = doc("\
glyph part-a 1 1
@@
glyph part-b 1 1
@@
exists part-([a-z])
glyph made-(x|y)-($1) 1 1 keep
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert_eq!(
        declared(&r),
        [
            "made-x-a", "made-x-b", "made-y-a", "made-y-b", "part-a", "part-b"
        ]
    );
}

/// The same for a `ref`: the target names one match, so a group on the header
/// no longer drags the slot along its own cycle.
#[test]
fn a_scoped_block_refs_the_one_match_its_names_were_built_from() {
    let d = doc("\
glyph part-a 1 1
@@
glyph part-b 1 1
@@
exists part-([a-z])
glyph made-(x|y)-($1) 1 1 keep
ref ($0)
");
    let r = Resolution::compute(&[&d]);
    let refs: Vec<(String, String)> = r
        .expansion
        .items()
        .filter_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.0.starts_with("made-") => {
                Some((name.0.clone(), body.refs[0].name.clone()))
            }
            _ => None,
        })
        .collect();
    let mut refs = refs;
    refs.sort();
    assert_eq!(
        refs,
        [
            ("made-x-a".to_string(), "part-a".to_string()),
            ("made-x-b".to_string(), "part-b".to_string()),
            ("made-y-a".to_string(), "part-a".to_string()),
            ("made-y-b".to_string(), "part-b".to_string()),
        ]
    );
}

/// A `glyph A = B` is one of the three items an `exists` may scope: it declares
/// a name, which is exactly what a search is for.
#[test]
fn a_scoped_alias_is_declared_per_match() {
    let d = doc("\
glyph part-a-k 1 1
@@
glyph part-b-k 1 1
@@
exists part-([a-z])-k
glyph part-($1) = ($0)
");
    let r = Resolution::compute(&[&d]);
    assert_eq!(errors(&r), Vec::<String>::new());
    assert_eq!(
        r.expansion.aliases.resolved_target("part-a"),
        Some("part-a-k")
    );
    assert_eq!(
        r.expansion.aliases.resolved_target("part-b"),
        Some("part-b-k")
    );
}

/// An alias a search declared is a name like any other, so another search finds
/// it — the fixpoint covers aliases as well as `glyph` headers.
#[test]
fn a_search_finds_what_a_scoped_alias_declared() {
    let d = doc("\
glyph part-a-k 1 1
@@
exists part-([a-z])-k
glyph part-($1) = ($0)
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

/// An alias is one line, so the carry is over after it — as for a `map`, and
/// unlike the `glyph` block whose first token it shares.
#[test]
fn the_carry_over_an_alias_lasts_one_line() {
    // The third line is neither an item start nor blank, so only an alias
    // having ended the scope on its own line leaves it ungoverned.
    let lines = ["exists part-(x)", "glyph part-($1) = ($0)", "# note"];
    let mut carry = Carry::default();
    let seen: Vec<Option<String>> = lines
        .iter()
        .map(|l| {
            carry.enter(l);
            carry.pattern().map(str::to_string)
        })
        .collect();
    assert_eq!(
        seen,
        [
            Some("part-(x)".to_string()),
            Some("part-(x)".to_string()),
            None
        ]
    );
}
