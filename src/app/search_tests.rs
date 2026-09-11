//! What the Search pane does, from the matcher up to the chords.
//!
//! A child module of [`super`] (declared through `#[path]`), so it still
//! reaches the private matchers; the scenarios that need a whole
//! `UniformApp` are gathered in `app_tests` at the bottom.

use super::*;

/// A source with no `name-parts` at all, for the walks that take the scoped
/// map (a slice-qualified line is read once per slice, with that slice's
/// bindings; see [`SliceNameParts::for_each_slice`]).
fn no_scoped_parts() -> SliceNameParts {
    SliceNameParts::default()
}

/// The point of the snapshot sources: a file no pane is editing is searched
/// without the filesystem being consulted at all. The path here exists
/// nowhere on disk, so any hit can only have come from memory.
#[test]
fn an_unopened_file_is_searched_from_the_snapshot_source() {
    let path = PathBuf::from("/nonexistent/never-read.unf");
    let source = "glyph foo 8 16\nref bar 0 0\n";
    let files = vec![(path.clone(), SearchText::Source(source))];
    let (hits, file_count) = collect_hits(
        &files,
        "bar",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert_eq!(file_count, 1);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, path);
    assert_eq!(hits[0].file_line, 2);
    assert_eq!(hits[0].text, "ref bar 0 0");
}

/// Declarations are listed before uses, and each group keeps the order the
/// files and their lines were walked in.
#[test]
fn declarations_are_listed_before_uses() {
    let one = "ref foo 0 0\nglyph foo 8 16\nmap A = foo\nglyph bar = foo\n";
    let two = "glyph foo = baz\n";
    let files = vec![
        (PathBuf::from("one.unf"), SearchText::Source(one)),
        (PathBuf::from("two.unf"), SearchText::Source(two)),
    ];
    let (hits, file_count) = collect_hits(
        &files,
        "foo",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert_eq!(file_count, 2);
    assert_eq!(
        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
        vec![
            "glyph foo 8 16",
            "glyph foo = baz",
            "ref foo 0 0",
            "map A = foo",
            "glyph bar = foo",
        ],
    );
    assert_eq!(
        hits.iter().map(|h| h.is_decl).collect::<Vec<_>>(),
        vec![true, true, false, false, false],
    );
}

/// A name an `exists` block declares is found at the block that declares
/// it, though the name occurs nowhere in the file. This is the whole reason
/// `match_spans` carries the search along: `glyph han-($1)` says nothing
/// about `han-4e00` on its own line.
#[test]
fn a_name_an_exists_block_declares_is_found_at_the_block() {
    let src = "glyph han-4e00:15x16 15 16\n\
               exists han-([0-9a-f]{4,5}):15x16\n\
               glyph han-($1) 16 16 advance 16\n\
               ref ($0) 1 0\n";
    let files = vec![(PathBuf::from("han.unf"), SearchText::Source(src))];
    let (hits, _) = collect_hits(
        &files,
        "han-4e00",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert_eq!(
        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
        vec!["glyph han-($1) 16 16 advance 16"],
    );
    assert!(hits[0].is_decl);
}

/// The same for a multi-alias: `han-k:15x16` is written nowhere, and the line
/// that names it is also an appearance of the `han.0:15x16` it stands for.
#[test]
fn a_name_a_multi_alias_declares_is_found_at_the_alias() {
    let src = "glyph han.0:15x16 15 16\n\
               glyph han-k:* = han.0:*\n";
    let files = vec![(PathBuf::from("han.unf"), SearchText::Source(src))];
    let search = |name: &str| {
        collect_hits(
            &files,
            name,
            SearchKind::Name(LinkTargetKind::Glyph),
            &no_scoped_parts(),
        )
        .0
        .iter()
        .map(|h| (h.text.clone(), h.is_decl))
        .collect::<Vec<_>>()
    };
    let alias = "glyph han-k:* = han.0:*".to_string();
    assert_eq!(search("han-k:15x16"), [(alias.clone(), true)]);
    assert_eq!(
        search("han.0:15x16"),
        [
            ("glyph han.0:15x16 15 16".to_string(), true),
            (alias, false)
        ],
    );
}

/// A `map`'s target is written with the `name-parts` its own `SLICE :`
/// qualifier binds, so the line is an appearance of one glyph per slice —
/// `triple-star` for `wide` and `triple-star-half` for `narrow`. Read with the
/// unqualified map alone the token expands to nothing and the line is an
/// appearance of neither, which is what made a `map` target behave unlike
/// every other glyph-name position.
#[test]
fn a_slice_qualified_map_target_is_an_appearance_of_each_slices_name() {
    let src = "name-parts wide : $-half = ``\n\
               name-parts narrow : $-half = -half\n\
               glyph triple-star 8 16\n\
               glyph triple-star-half 8 16\n\
               map wide|narrow : ⁂ = triple-star($-half)\n";
    let doc = crate::document_io::parse_document_from_str(src, PathBuf::from("s.unf")).unwrap();
    let refs = [&doc];
    let scoped = SliceNameParts::with_base(&refs, crate::document::collect_name_parts(&refs));
    let files = vec![(PathBuf::from("s.unf"), SearchText::Source(src))];
    for name in ["triple-star", "triple-star-half"] {
        let (hits, _) = collect_hits(
            &files,
            name,
            SearchKind::Name(LinkTargetKind::Glyph),
            &scoped,
        );
        assert_eq!(
            hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            vec![
                format!("glyph {name} 8 16").as_str(),
                "map wide|narrow : ⁂ = triple-star($-half)",
            ],
            "{name} is declared once and used by the map line",
        );
    }
}

/// And the `ref` inside that block is a *use* of what the search matched,
/// which is why the carry outlives the header line.
#[test]
fn a_capture_ref_is_a_use_of_the_name_the_search_matched() {
    let src = "glyph han-4e00:15x16 15 16\n\
               exists han-([0-9a-f]{4,5}):15x16\n\
               glyph han-($1) 16 16 advance 16\n\
               ref ($0) 1 0\n";
    let files = vec![(PathBuf::from("han.unf"), SearchText::Source(src))];
    let (hits, _) = collect_hits(
        &files,
        "han-4e00:15x16",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert_eq!(
        hits.iter()
            .map(|h| (h.text.as_str(), h.is_decl))
            .collect::<Vec<_>>(),
        vec![
            ("glyph han-4e00:15x16 15 16", true),
            ("ref ($0) 1 0", false)
        ],
    );
}

/// The carry ends where the block does: a `$1` under the *next* block is
/// not the previous search's.
#[test]
fn the_search_does_not_reach_past_the_block_it_governs() {
    let src = "exists han-([0-9a-f]{4,5}):15x16\n\
               glyph han-($1) 16 16\n\
               ref ($0) 1 0\n\
               glyph other-($1) 8 16\n";
    let files = vec![(PathBuf::from("han.unf"), SearchText::Source(src))];
    let (hits, _) = collect_hits(
        &files,
        "other-4e00",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert!(
        hits.is_empty(),
        "{:?}",
        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
    );
}

/// A path the snapshot has no source for contributes nothing rather than
/// sending the search back to disk for it.
#[test]
fn a_file_with_no_source_contributes_no_hits() {
    let files: Vec<(PathBuf, SearchText)> = Vec::new();
    let (hits, file_count) = collect_hits(
        &files,
        "bar",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert!(hits.is_empty());
    assert_eq!(file_count, 0);
}

/// Start columns only; the spans' ends are pinned separately, by the
/// highlight tests.
fn cols(line: &str, name: &str, kind: LinkTargetKind) -> Vec<usize> {
    cols_with(line, name, kind, &NamePartsMap::default())
}

fn cols_with(line: &str, name: &str, kind: LinkTargetKind, parts: &NamePartsMap) -> Vec<usize> {
    match_spans(line, name, kind, None, None, &[], parts)
        .into_iter()
        .map(|s| s.col_start)
        .collect()
}

#[test]
fn glyph_name_is_found_where_it_is_defined_and_used() {
    assert_eq!(
        cols("glyph foo 8 16", "foo", LinkTargetKind::Glyph),
        vec![6]
    );
    assert_eq!(cols("ref foo 0 0", "foo", LinkTargetKind::Glyph), vec![4]);
    assert_eq!(cols("map A = foo", "foo", LinkTargetKind::Glyph), vec![8]);
    assert_eq!(
        cols("glyph bar = foo", "foo", LinkTargetKind::Glyph),
        vec![12]
    );
    assert_eq!(
        cols("remap liga : foo -> bar", "foo", LinkTargetKind::Glyph),
        vec![13],
    );
    assert_eq!(
        cols("assert same foo bar", "foo", LinkTargetKind::Glyph),
        vec![12],
    );
}

/// A name written as a pattern is an appearance of every name it denotes,
/// wherever the pattern stands — the definition, a `ref`, an operand.
#[test]
fn a_pattern_token_that_denotes_the_name_is_an_appearance() {
    for (line, col) in [
        ("glyph fo(o|q) 8 16", 6),
        ("ref fo(o|q) 0 0", 4),
        ("remap liga : fo(o|q) -> bar", 13),
        ("glyph foo|bar 8 16", 6),
    ] {
        assert_eq!(
            cols(line, "foo", LinkTargetKind::Glyph),
            vec![col],
            "{line}"
        );
    }
    assert_eq!(
        cols(
            "glyph uni($#0041..0043) 8 16",
            "uni0042",
            LinkTargetKind::Glyph
        ),
        vec![6],
    );
}

/// The cyclic expansion is what decides it: `(a|b)-(1|2)` is `a-1` and
/// `b-2`, so `a-2` is not one of its names and its line is not a hit.
#[test]
fn a_pattern_that_does_not_denote_the_name_is_not_an_appearance() {
    assert!(cols("glyph fo(p|q) 8 16", "foo", LinkTargetKind::Glyph).is_empty());
    assert!(cols("glyph (a|b)-(1|2) 2 2", "a-2", LinkTargetKind::Glyph).is_empty());
    assert_eq!(
        cols("glyph (a|b)-(1|2) 2 2", "b-2", LinkTargetKind::Glyph),
        vec![6]
    );
}

/// A pattern spelled with a `$var` denotes what the name parts say it
/// does, so the search has to substitute them exactly as the pipeline does.
#[test]
fn a_name_part_is_substituted_before_the_pattern_is_matched() {
    let mut parts = NamePartsMap::default();
    parts.insert("$init".to_string(), vec!["g".to_string(), "n".to_string()]);
    assert_eq!(
        cols_with(
            "glyph hangul-($init) 8 16",
            "hangul-n",
            LinkTargetKind::Glyph,
            &parts
        ),
        vec![6],
    );
    assert!(
        cols_with(
            "glyph hangul-($init) 8 16",
            "hangul-d",
            LinkTargetKind::Glyph,
            &parts
        )
        .is_empty()
    );
    // With no parts in force the reference expands to nothing, and a
    // pattern that denotes no name is no appearance.
    assert!(
        cols(
            "glyph hangul-($init) 8 16",
            "hangul-n",
            LinkTargetKind::Glyph
        )
        .is_empty()
    );
}

/// A `$-N` names a group of the header above it, so the `ref` line has to
/// be walked with that header in force — on its own it denotes nothing.
#[test]
fn a_back_reference_is_read_against_the_header_above_it() {
    let name_parts = NamePartsMap::default();
    let mut captures: Vec<Vec<String>> = Vec::new();
    advance_block_captures(&mut captures, "glyph out-(a|b|c) 8 16", None, &name_parts);

    let hit = |name: &str| {
        match_spans(
            "  ref dep-($-1) 0 0",
            name,
            LinkTargetKind::Glyph,
            None,
            None,
            &captures,
            &name_parts,
        )
        .len()
    };
    assert_eq!(hit("dep-b"), 1);
    assert_eq!(hit("dep-z"), 0);
    // With no header in force there is nothing for it to name.
    assert_eq!(
        cols("  ref dep-($-1) 0 0", "dep-b", LinkTargetKind::Glyph),
        Vec::<usize>::new(),
    );
}

/// An alias and a `map` write their pattern and name it again on the same
/// line, so they bind their own groups rather than the block's.
#[test]
fn a_line_that_writes_its_own_pattern_binds_its_own_groups() {
    assert_eq!(
        cols(
            "glyph out-(a|b) = dep-($-1)",
            "dep-b",
            LinkTargetKind::Glyph
        ),
        vec![18],
    );
    assert_eq!(
        cols("map (A|B) = dep-($-1)", "dep-B", LinkTargetKind::Glyph),
        vec![12],
    );
}

/// The whole pattern token is the span, so the pane highlights what the
/// line actually says rather than the name that was searched for.
#[test]
fn a_pattern_hit_highlights_the_whole_pattern_token() {
    let line = "    ref fo(o|q) 0 0";
    let span = match_spans(
        line,
        "foo",
        LinkTargetKind::Glyph,
        None,
        None,
        &[],
        &NamePartsMap::default(),
    )[0];
    let h = hit(std::path::Path::new("a.unf"), 0, 1, line, span);
    assert_eq!(&h.text[h.highlight.0..h.highlight.1], "fo(o|q)");
}

/// The cheap filters in front decide whether a line is tokenized at all,
/// and a pixel row must still be rejected — `(`, `|` and `*` are shape
/// codes as much as they are pattern syntax.
#[test]
fn only_a_keyword_line_can_be_carrying_a_pattern() {
    assert!(may_write_a_pattern("glyph fo(o|q) 8 16"));
    assert!(may_write_a_pattern("  ref hangul-($init) 0 0"));
    assert!(may_write_a_pattern("assume unused foo*3"));
    assert!(!may_write_a_pattern("(((|.@@bb"));
    assert!(!may_write_a_pattern("glyph foo 8 16"));
    assert!(!may_write_a_pattern("color red = #ff0000"));
}

#[test]
fn glyph_search_does_not_match_partial_names_or_comments() {
    assert!(cols("glyph foobar 8 16", "foo", LinkTargetKind::Glyph).is_empty());
    assert!(cols("ref foo-ext 0 0", "foo", LinkTargetKind::Glyph).is_empty());
    assert!(cols("ref bar 0 0 // foo", "foo", LinkTargetKind::Glyph).is_empty());
}

/// A remap group and a glyph can share a name; they are different things,
/// and a glyph search must not list the group.
#[test]
fn a_remap_group_is_not_a_glyph_name() {
    assert!(cols("remap foo : a -> b", "foo", LinkTargetKind::Glyph).is_empty());
    assert_eq!(
        cols("remap foo : a -> b", "foo", LinkTargetKind::Remap),
        vec![6]
    );
    assert_eq!(
        cols("feature liga for latn : foo", "foo", LinkTargetKind::Remap),
        vec![24],
    );
}

#[test]
fn a_feature_tag_is_found_on_every_declaration() {
    assert_eq!(
        cols("feature ccmp for latn : g", "ccmp", LinkTargetKind::Feature),
        vec![8],
    );
    assert_eq!(
        cols(
            "feature ccmp for cyrl/SRB : g",
            "ccmp",
            LinkTargetKind::Feature
        ),
        vec![8],
    );
    // The group it points at is not the tag.
    assert!(
        cols(
            "feature liga for latn : ccmp",
            "ccmp",
            LinkTargetKind::Feature
        )
        .is_empty()
    );
}

/// Both signs of an anchor are the same anchor, and the anchor-driven
/// `feature` variant names one too.
#[test]
fn an_anchor_is_found_through_both_signs() {
    assert_eq!(
        cols("anchor +above 4 1", "above", LinkTargetKind::Anchor),
        vec![7]
    );
    assert_eq!(
        cols("anchor -above 2 1", "above", LinkTargetKind::Anchor),
        vec![7]
    );
    assert_eq!(
        cols(
            "feature abvm for hang : anchor above",
            "above",
            LinkTargetKind::Anchor
        ),
        vec![31],
    );
}

#[test]
fn a_name_parts_variable_is_found_inside_the_names_it_builds() {
    assert_eq!(
        cols(
            "name-parts $init = a b c",
            "$init",
            LinkTargetKind::NameParts
        ),
        vec![11],
    );
    assert_eq!(
        cols(
            "name-parts $combo = $init $final",
            "$init",
            LinkTargetKind::NameParts
        ),
        vec![20],
    );
    assert_eq!(
        cols(
            "glyph hangul-($init)-l 8 16",
            "$init",
            LinkTargetKind::NameParts
        ),
        vec![14],
    );
    assert_eq!(
        cols("ref hangul-$init 0 0", "$init", LinkTargetKind::NameParts),
        vec![11],
    );
    // No partial matches: `$initial` is a different variable.
    assert!(
        cols(
            "ref hangul-$initial 0 0",
            "$init",
            LinkTargetKind::NameParts
        )
        .is_empty()
    );
}

#[test]
fn a_color_is_found_at_its_definition_and_its_uses() {
    assert_eq!(
        cols("color red = #ff0000", "red", LinkTargetKind::Color),
        vec![6]
    );
    assert_eq!(
        cols("color light-red = red", "red", LinkTargetKind::Color),
        vec![18],
    );
    assert_eq!(
        cols("ref foo 0 0 fill red", "red", LinkTargetKind::Color),
        vec![17],
    );
}

#[test]
fn several_appearances_on_one_line_are_all_reported() {
    assert_eq!(
        cols("glyph foo = foo", "foo", LinkTargetKind::Glyph),
        vec![6, 12],
    );
}

/// The pane shows the line trimmed, so the highlight has to move with it.
#[test]
fn the_highlight_follows_the_trimmed_text() {
    let line = "    ref foo 0 0";
    let span = match_spans(
        line,
        "foo",
        LinkTargetKind::Glyph,
        None,
        None,
        &[],
        &NamePartsMap::default(),
    )[0];
    let h = hit(std::path::Path::new("a.unf"), 0, 3, line, span);
    assert_eq!(h.text, "ref foo 0 0");
    assert_eq!(&h.text[h.highlight.0..h.highlight.1], "foo");
}

/// The span is the *written* token, so an anchor's sign and a quoted
/// token's backticks are highlighted with it — what is picked out is what
/// the line actually says, not a reconstruction of the bare name.
#[test]
fn the_highlight_covers_the_token_as_written() {
    for (line, name, kind, expected) in [
        (
            "anchor +above 4 1",
            "above",
            LinkTargetKind::Anchor,
            "+above",
        ),
        (
            "ref `foo bar` 0 0",
            "foo bar",
            LinkTargetKind::Glyph,
            "`foo bar`",
        ),
        (
            "glyph x-$init 2 2",
            "$init",
            LinkTargetKind::NameParts,
            "$init",
        ),
    ] {
        let span = *match_spans(line, name, kind, None, None, &[], &NamePartsMap::default())
            .first()
            .unwrap_or_else(|| panic!("no match in {line:?}"));
        let h = hit(std::path::Path::new("a.unf"), 0, 1, line, span);
        assert_eq!(&h.text[h.highlight.0..h.highlight.1], expected, "{line:?}");
    }
}

/// Two occurrences on one line are two rows, each highlighting its own.
#[test]
fn each_row_highlights_its_own_occurrence() {
    let line = "glyph foo = foo";
    let spans = match_spans(
        line,
        "foo",
        LinkTargetKind::Glyph,
        None,
        None,
        &[],
        &NamePartsMap::default(),
    );
    assert_eq!(spans.len(), 2);
    let hits: Vec<_> = spans
        .into_iter()
        .enumerate()
        .map(|(i, s)| hit(std::path::Path::new("a.unf"), i, 1, line, s))
        .collect();
    assert_eq!(hits[0].highlight, (6, 9));
    assert_eq!(hits[1].highlight, (12, 15));
}

#[test]
fn hits_run_over_a_document_in_order_and_skip_pixel_grids() {
    use crate::document::PixelGrid;
    let lines = vec![
        DocLine::Text("glyph foo 2 2".to_string()),
        DocLine::Grid(PixelGrid::new(2, 2)),
        DocLine::Text("ref foo 0 0".to_string()),
        DocLine::Text("map A = foo".to_string()),
    ];
    assert_eq!(
        hits_in_doclines(
            &lines,
            "foo",
            SearchKind::Name(LinkTargetKind::Glyph),
            &no_scoped_parts()
        )
        .into_iter()
        .map(|(i, s)| (i, s.col_start, s.col_end, s.is_decl))
        .collect::<Vec<_>>(),
        vec![(0, 6, 9, true), (2, 4, 7, false), (3, 8, 11, false)],
    );
}

/// A glyph written with `@` is an appearance of the name it expands to, so
/// the Search pane lists it beside the full-name ones. The literal filters
/// in front cannot hide it: `may_write_an_at_name` is what lets an `@` line
/// through, and a pixel row — where `@@` is the full-ink code — still does
/// not pay for a tokenizing pass.
#[test]
fn an_at_name_is_an_appearance_of_what_it_expands_to() {
    let path = PathBuf::from("/nonexistent/never-read.unf");
    let source = "glyph foo\nref @-bar\nglyph @-bar\nmap A = foo-bar\n";
    let files = vec![(path, SearchText::Source(source))];
    let (hits, _) = collect_hits(
        &files,
        "foo-bar",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert_eq!(
        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
        // The `glyph` line is a declaration and so is listed first.
        vec!["glyph @-bar", "ref @-bar", "map A = foo-bar"],
    );
    // And the base itself is not one of its own family's appearances.
    let files = vec![(PathBuf::from("x.unf"), SearchText::Source(source))];
    let (hits, _) = collect_hits(
        &files,
        "foo",
        SearchKind::Name(LinkTargetKind::Glyph),
        &no_scoped_parts(),
    );
    assert_eq!(
        hits.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
        vec!["glyph foo"],
    );
}

#[test]
fn only_a_token_start_counts_as_an_at_name() {
    assert!(may_write_an_at_name("ref @-bar"));
    assert!(may_write_an_at_name("glyph `@ odd`"));
    // A pixel row is all shape codes, and `@@` is one of them.
    assert!(!may_write_an_at_name("@@..@@.."));
    assert!(!may_write_an_at_name("glyph foo"));
}

/// The dropdown is a cycle in both directions, and a kind only a
/// Ctrl/Cmd+click can set steps *into* it rather than swallowing the chord.
#[test]
fn the_kind_cycle_wraps_at_both_ends() {
    let text = SearchKind::Text;
    let glyph = SearchKind::Name(LinkTargetKind::Glyph);
    assert_eq!(SearchKind::CHOICES, [text, glyph]);

    assert_eq!(text.cycled(true), glyph);
    assert_eq!(glyph.cycled(true), text);
    assert_eq!(glyph.cycled(false), text);
    assert_eq!(text.cycled(false), glyph);

    // An anchor search is not one of the choices.
    let anchor = SearchKind::Name(LinkTargetKind::Anchor);
    assert_eq!(anchor.cycled(true), text);
    assert_eq!(anchor.cycled(false), glyph);
}

/// A text search is verbatim, and its rows come file by file in the order
/// the files are handed over — which the caller sorts by path, so the first
/// row is in the file whose name sorts first.
#[test]
fn a_text_search_is_verbatim_and_lists_every_occurrence() {
    let files = vec![
        (
            PathBuf::from("a.unf"),
            SearchText::Source("// early bird\nmeta height 16\n"),
        ),
        (
            PathBuf::from("b.unf"),
            SearchText::Source("meta ascent 14\n// the early bird, twice: early\n"),
        ),
    ];
    let (hits, file_count) = collect_hits(&files, "early", SearchKind::Text, &no_scoped_parts());
    assert_eq!(file_count, 2);
    assert_eq!(
        hits.iter()
            .map(|h| (
                h.path.file_name().unwrap().to_str().unwrap(),
                h.file_line,
                h.ordinal,
                h.highlight
            ))
            .collect::<Vec<_>>(),
        vec![
            ("a.unf", 1, 0, (3, 8)),
            // Two occurrences on one line are two rows, and the second is
            // this file's ordinal 1 — the columns are the trimmed line's.
            ("b.unf", 2, 0, (7, 12)),
            ("b.unf", 2, 1, (26, 31)),
        ],
    );

    // Verbatim: no case folding, and no collapsing of the space.
    for query in ["Early", "early  bird"] {
        let (hits, _) = collect_hits(&files, query, SearchKind::Text, &no_scoped_parts());
        assert!(hits.is_empty(), "'{query}' should not match");
    }
}

/// A text search does not reach into a glyph's pixel rows — they are one
/// grid line to the caret, so a row matched there could not be jumped to —
/// and the two ends of the search agree about that, which is what keeps the
/// ordinals of everything after a grid lined up.
#[test]
fn a_grid_is_skipped_by_both_ends_of_a_text_search() {
    let source = "glyph a 2 2\n@@..\n..@@\n// @@ mentioned in prose\nmeta height 2\n";
    let path = PathBuf::from("a.unf");
    let doclines = crate::document_io::parse_doclines(source);
    let document = Document::new(path.clone());

    let (from_source, _) = collect_hits(
        &[(path.clone(), SearchText::Source(source))],
        "@@",
        SearchKind::Text,
        &no_scoped_parts(),
    );
    assert_eq!(
        from_source
            .iter()
            .map(|h| (h.file_line, h.ordinal, h.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(4, 0, "// @@ mentioned in prose")],
        "the two pixel rows are not lines a search can match"
    );

    // And the open buffer finds the same one occurrence at the same
    // ordinal, which is the number `goto_search_hit` re-finds it by.
    let (from_buffer, _) = collect_hits(
        &[(path, SearchText::Buffer(&doclines, &document))],
        "@@",
        SearchKind::Text,
        &no_scoped_parts(),
    );
    assert_eq!(
        from_buffer
            .iter()
            .map(|h| (h.ordinal, h.text.as_str()))
            .collect::<Vec<_>>(),
        vec![(0, "// @@ mentioned in prose")],
    );
}
/// The pane driven the way a reader drives it: the chords, the box, and where
/// the caret ends up. Everything below the UI is covered above; these are the
/// scenarios that only exist once the two are wired together.
mod app_tests {
    use super::*;
    use crate::app::UniformApp;
    use crate::app::background::startup_tests::TempDir;
    use crate::app::settings::Settings;

    /// A directory with two files, `a.unf` sorting before `b.unf`, each naming
    /// the other's glyph so that either could be "the first hit" if the order
    /// were wrong.
    fn two_file_app(tag: &str) -> (TempDir, egui::Context, UniformApp) {
        let dir = TempDir::new(tag);
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\nglyph alpha 2 2\n@@..\n..@@\n",
        )
        .unwrap();
        std::fs::write(
            dir.0.join("b.unf"),
            "glyph beta\nref alpha 0 0\n\n// alpha again, in prose\n",
        )
        .unwrap();
        let ctx = egui::Context::default();
        let app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        (dir, ctx, app)
    }

    /// Feeds one key press through a real input pass, which is the only place
    /// `consume_key` can read it, and returns the deferred Ctrl/Cmd+G step.
    fn press(
        app: &mut UniformApp,
        ctx: &egui::Context,
        key: egui::Key,
        modifiers: egui::Modifiers,
    ) -> Option<bool> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1000.0, 800.0),
            )),
            modifiers,
            events: vec![egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }],
            ..Default::default()
        };
        ctx.begin_pass(raw);
        let step = app.handle_search_keys(ctx);
        let _ = ctx.end_pass();
        step
    }

    fn cmd() -> egui::Modifiers {
        egui::Modifiers::COMMAND
    }

    fn cmd_shift() -> egui::Modifiers {
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
    }

    /// Where the caret is, as `(file name, line)`.
    fn caret(app: &UniformApp) -> (String, usize) {
        let idx = app.panes.active_doc_idx().expect("no document is showing");
        let doc = &app.open_documents[idx];
        (
            doc.document
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
            doc.editor_state.cursor_line(),
        )
    }

    /// The first press opens the pane on the kind the chord names; pressing it
    /// again — the box now holding the keyboard — steps the dropdown instead,
    /// forward without Shift and back with it, wrapping at both ends.
    #[test]
    fn ctrl_f_focuses_the_box_and_then_cycles_the_kind() {
        let (_dir, ctx, mut app) = two_file_app("search-ctrl-f");
        let glyph = SearchKind::Name(LinkTargetKind::Glyph);

        press(&mut app, &ctx, egui::Key::F, cmd());
        assert_eq!(app.search.kind, SearchKind::Text);
        assert!(app.search.focus_query, "the box has to take the keyboard");
        assert_eq!(app.bottom_panel_tab, Some(super::super::panels::SEARCH_TAB));

        // Shift picks the glyph kind outright rather than stepping.
        app.search.query_focused = false;
        press(&mut app, &ctx, egui::Key::F, cmd_shift());
        assert_eq!(app.search.kind, glyph);

        // With the box focused the chord steps, and wraps at each end.
        app.search.query_focused = true;
        press(&mut app, &ctx, egui::Key::F, cmd());
        assert_eq!(app.search.kind, SearchKind::Text);
        press(&mut app, &ctx, egui::Key::F, cmd());
        assert_eq!(app.search.kind, glyph);
        press(&mut app, &ctx, egui::Key::F, cmd_shift());
        assert_eq!(app.search.kind, SearchKind::Text);
        press(&mut app, &ctx, egui::Key::F, cmd_shift());
        assert_eq!(app.search.kind, glyph);

        // The box must keep the keyboard through all of it.
        assert!(app.search.focus_query);
    }

    /// Enter in the box runs the search and lands on the first hit, which for
    /// a text search is in the file whose name sorts first — even though
    /// `b.unf` is the file that was already open.
    #[test]
    fn a_text_search_lands_on_the_first_hit_by_file_name() {
        let (dir, ctx, mut app) = two_file_app("search-text-first");
        app.open_file(dir.0.join("b.unf"));

        app.search.kind = SearchKind::Text;
        app.search.query = "alpha".to_string();
        app.run_search_from_box(&ctx);

        assert_eq!(app.search.counter(), "1/3");
        assert_eq!(
            caret(&app),
            ("a.unf".to_string(), 4),
            "`glyph alpha` in a.unf"
        );
        assert_eq!(app.search.message, None);
        // The header row's own summary carries only what the counter and the
        // box do not already say.
        assert_eq!(app.search.summary().as_deref(), Some("in 2 files"));
    }

    /// The kind-cycling press must not disturb a box the reader is already
    /// typing in, but a press that *brings* the focus hands over a selected
    /// query so the next keystroke replaces it.
    #[test]
    fn the_query_is_selected_only_when_the_focus_arrives() {
        let (_dir, ctx, mut app) = two_file_app("search-select");
        app.search.query = "alpha".to_string();

        app.search.query_focused = false;
        press(&mut app, &ctx, egui::Key::F, cmd());
        assert!(app.search.focus_query);
        assert!(app.search.select_query, "typing should replace the query");

        // Cycling the kind of a box that already holds the keyboard leaves the
        // caret where the reader put it.
        app.search.select_query = false;
        app.search.query_focused = true;
        press(&mut app, &ctx, egui::Key::F, cmd());
        assert!(app.search.focus_query);
        assert!(!app.search.select_query);
    }

    /// A glyph search puts the declaration first, so Ctrl/Cmd+Shift+F, a name
    /// and Enter is "go to that glyph" — the pane's whole reason for taking a
    /// kind rather than only ever matching text.
    #[test]
    fn a_glyph_search_lands_on_the_declaration() {
        let (dir, ctx, mut app) = two_file_app("search-glyph-decl");
        app.open_file(dir.0.join("b.unf"));

        app.search.kind = SearchKind::Name(LinkTargetKind::Glyph);
        app.search.query = "alpha".to_string();
        app.run_search_from_box(&ctx);

        assert_eq!(caret(&app), ("a.unf".to_string(), 4));
        // The prose mention is not an appearance of the name, so a glyph
        // search finds one fewer than the text search above.
        assert_eq!(app.search.counter(), "1/2");
    }

    /// Nothing found is a message and nothing else: the caret does not move,
    /// and the box keeps the keyboard so the query can be corrected.
    #[test]
    fn a_search_with_no_match_moves_nothing() {
        let (dir, ctx, mut app) = two_file_app("search-no-match");
        app.open_file(dir.0.join("b.unf"));
        let before = caret(&app);

        app.search.kind = SearchKind::Text;
        app.search.query = "nowhere at all".to_string();
        app.run_search_from_box(&ctx);

        assert_eq!(caret(&app), before);
        assert_eq!(app.search.counter(), "–/0");
        assert_eq!(
            app.search.message.as_deref(),
            Some("No match for 'nowhere at all'")
        );
        assert_eq!(app.search.summary(), None, "no files to count");
        assert!(app.search.focus_query);
    }

    /// Ctrl/Cmd+G walks the list the pane shows and comes round at both ends.
    #[test]
    fn ctrl_g_steps_through_the_hits_and_wraps() {
        let (dir, ctx, mut app) = two_file_app("search-step");
        app.open_file(dir.0.join("b.unf"));
        app.search.kind = SearchKind::Text;
        app.search.query = "alpha".to_string();
        app.run_search_from_box(&ctx);
        assert_eq!(app.search.counter(), "1/3");

        // The chord is read now and made after the frame's editors have run,
        // which is what `handle_search_keys` hands back.
        for expected in ["2/3", "3/3", "1/3"] {
            let step = press(&mut app, &ctx, egui::Key::G, cmd());
            app.step_search_hit(&ctx, step.expect("Ctrl/Cmd+G was not read"));
            assert_eq!(app.search.counter(), expected);
        }
        for expected in ["3/3", "2/3"] {
            let step = press(&mut app, &ctx, egui::Key::G, cmd_shift());
            assert_eq!(step, Some(false));
            app.step_search_hit(&ctx, false);
            assert_eq!(app.search.counter(), expected);
        }
        assert_eq!(
            caret(&app),
            ("b.unf".to_string(), 1),
            "`ref alpha` in b.unf"
        );
    }

    /// An edit made after a search does not move the rows the pane is still
    /// showing, so a click on one has to land on the line that row *displays*
    /// — not on whatever now sits at the ordinal the row was recorded with.
    /// Inserting one more occurrence above the others used to push every later
    /// click one entry off.
    #[test]
    fn a_click_lands_on_the_listed_line_after_the_file_was_edited() {
        let (dir, ctx, mut app) = two_file_app("search-after-edit");
        app.open_file(dir.0.join("b.unf"));
        app.search.kind = SearchKind::Text;
        app.search.query = "alpha".to_string();
        app.run_search_from_box(&ctx);

        // The last row is b.unf's prose mention; jump to it once, as a reader
        // would before going back to edit.
        let last = app.search.hits().len() - 1;
        app.goto_search_hit(&ctx, last);
        assert_eq!(caret(&app), ("b.unf".to_string(), 3));

        // An edit that adds an earlier occurrence to the same file.
        let idx = app.panes.active_doc_idx().unwrap();
        app.open_documents[idx]
            .lines
            .insert(0, DocLine::Text("// alpha inserted".to_string()));

        app.goto_search_hit(&ctx, last);
        assert_eq!(
            caret(&app),
            ("b.unf".to_string(), 4),
            "the prose mention, one line further down than it was"
        );
    }

    /// Escape in the box is a focus move and nothing more — the results and the
    /// place in them survive, so a Ctrl/Cmd+G from the editor carries on.
    #[test]
    fn escape_in_the_box_only_gives_the_keyboard_back() {
        let (dir, ctx, mut app) = two_file_app("search-escape");
        app.open_file(dir.0.join("b.unf"));
        app.search.kind = SearchKind::Text;
        app.search.query = "alpha".to_string();
        app.run_search_from_box(&ctx);
        app.search.query_focused = true;

        press(&mut app, &ctx, egui::Key::Escape, egui::Modifiers::NONE);
        assert!(!app.search.query_focused);
        assert_eq!(app.search.query, "alpha", "Escape cancels nothing");
        assert_eq!(app.search.counter(), "1/3");

        let step = press(&mut app, &ctx, egui::Key::G, cmd());
        app.step_search_hit(&ctx, step.unwrap());
        assert_eq!(app.search.counter(), "2/3");
    }
}
