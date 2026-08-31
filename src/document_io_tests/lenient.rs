//! Intermediate editing states: what the lenient derive tolerates, what the
//! strict parse rejects, and the box flags a header may state.

use super::*;

#[test]
fn derive_empty_body_glyph() {
    let input = "glyph foo\n";
    let lines = parse_doclines(input);
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert!(body.pixels.is_none());
        assert!(body.refs.is_empty());
    } else {
        panic!("expected glyph");
    }
}

#[test]
fn derive_glyph_header_split_from_alias() {
    let input = "glyph foo\n= bar\n";
    let lines = parse_doclines(input);
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 2);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert!(body.pixels.is_none());
        assert!(body.refs.is_empty());
    } else {
        panic!("expected glyph at item 0");
    }
    assert!(matches!(doc.items[1], DocumentItem::Directive(_)));
}

#[test]
fn derive_glyph_with_dims_no_grid_docline() {
    // Simulates editing state: header with dims but Grid DocLine removed
    let lines = vec![DocLine::Text("glyph foo 8 16".to_string())];
    let (doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(doc.items.len(), 1);
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        let grid = body.pixels.as_ref().expect("should have empty grid");
        assert_eq!(grid.width, 8);
        assert_eq!(grid.height, 16);
        assert!(grid.is_all_empty());
        assert!(body.refs.is_empty());
    } else {
        panic!("expected glyph");
    }
}

#[test]
fn docline_roundtrip_all_directive_types() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

// a comment
name-parts $base = stem wide

glyph stem 2 2
@@@@
..@@

glyph wide 3 1
@@..@@

glyph alias = stem

glyph comp
ref stem
ref wide 1 0
anchor -join 0 0
anchor +join 2 0

glyph batch
ref stem-(a|b)

glyph keep-empty keep advance 0

map A = stem
map B = wide
remap set1 : stem -> wide
remap liga1 : stem wide -> batch
remap liga2 : stem wide -> batch stem
feature liga for latn : set1
exclude-from-sample stem
";
    let lines = parse_doclines(input);
    let mut output = Vec::new();
    serialize_doclines(&lines, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert_eq!(input, output_str, "DocLine round-trip failed");

    let old_doc = parse_document_from_str(input, "test.unf".into()).unwrap();
    let (new_doc, _) = derive_document(&lines, "test.unf".into()).unwrap();
    assert_eq!(
        old_doc.items.len(),
        new_doc.items.len(),
        "item count mismatch"
    );
}

#[test]
fn strict_parse_rejects_partial_glyph_header() {
    let input = "glyph x 2 nope\n..@@\n";
    assert!(parse_document_from_str(input, "test.unf".into()).is_err());
}

#[test]
fn strict_parse_accepts_valid_glyph_headers() {
    for input in [
        "glyph foo\n",
        "glyph foo 2 1\n..@@\n",
        "glyph foo keep\n",
        "glyph foo 2 1 keep\n..@@\n",
        "glyph foo advance 5\n",
        "glyph foo origin -1 2\n",
        "glyph foo 2 1 keep advance 5 origin -1 2\n..@@\n",
        "glyph foo 2 1 desync\n..@@\n",
        "glyph foo desync 2 1\n..@@\n",
        "glyph foo 2 1 vectoronly\n..@@\n",
        "glyph foo vectoronly 2 1\n..@@\n",
        "glyph foo = bar\n",
    ] {
        assert!(
            parse_document_from_str(input, "test.unf".into()).is_ok(),
            "should accept: {input:?}"
        );
    }
}

/// `desync` says the grid that follows is bitmap ink only, so a header
/// carrying it still owns a pixel grid — reconciliation and the document model
/// have to agree about that, whichever side of `W H` the flag sits on.
#[test]
fn desync_header_owns_its_pixel_grid_and_round_trips() {
    for input in [
        "glyph foo 2 1 desync\n..@@\n",
        "glyph foo desync 2 1\n..@@\n",
    ] {
        let tokens = tokenize_tokens(input.lines().next().unwrap()).unwrap();
        assert_eq!(
            glyph_header_dims(&tokens[1..]).map(|d| (d.width, d.height)),
            Some((2, 1)),
            "desync header should still own a grid: {input:?}"
        );

        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {:?}", doc.items[0]);
        };
        assert!(body.desync, "desync should reach the body: {input:?}");
        assert!(body.pixels.is_some(), "the grid is still parsed: {input:?}");

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "glyph foo 2 1 desync\n..@@\n",
        );
    }
}

/// `vectoronly` is `desync`'s mirror and shares its shape: a keyword flag on
/// either side of `W H`, on a header that still owns the grid below it.
#[test]
fn vectoronly_header_owns_its_pixel_grid_and_round_trips() {
    for input in [
        "glyph foo 2 1 vectoronly\n..@@\n",
        "glyph foo vectoronly 2 1\n..@@\n",
    ] {
        let tokens = tokenize_tokens(input.lines().next().unwrap()).unwrap();
        assert_eq!(
            glyph_header_dims(&tokens[1..]).map(|d| (d.width, d.height)),
            Some((2, 1)),
            "vectoronly header should still own a grid: {input:?}"
        );

        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {:?}", doc.items[0]);
        };
        assert!(
            body.vectoronly,
            "vectoronly should reach the body: {input:?}"
        );
        assert!(body.pixels.is_some(), "the grid is still parsed: {input:?}");

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "glyph foo 2 1 vectoronly\n..@@\n",
        );
    }
}

/// The two flags name opposite drawings — one takes the grid out of the vector
/// build, the other puts the vector drawing into the bitmap one — so a header
/// asking for both is rejected rather than resolved in some order.
#[test]
fn desync_and_vectoronly_together_are_rejected() {
    assert!(
        parse_document_from_str("glyph foo 2 1 desync vectoronly\n..@@\n", "test.unf".into())
            .is_err(),
        "desync + vectoronly should not parse",
    );
}

/// `origin C R` and `extent W H` are the only two-valued header flags, so the
/// parser has to take both components before it hands the walker back — and a
/// bare `W H` pair on the same line must still be the grid's, not theirs.
#[test]
fn origin_and_extent_parse_in_any_order_and_round_trip() {
    for input in [
        "glyph foo 2 1 origin -1 3 extent 4 5\n..@@\n",
        "glyph foo origin -1 3 2 1 extent 4 5\n..@@\n",
        "glyph foo extent 4 5 origin -1 3 2 1\n..@@\n",
    ] {
        let tokens = tokenize_tokens(input.lines().next().unwrap()).unwrap();
        assert_eq!(
            glyph_header_dims(&tokens[1..]).map(|d| (d.width, d.height)),
            Some((2, 1)),
            "the grid's own W H must not be eaten by a two-valued flag: {input:?}"
        );

        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {:?}", doc.items[0]);
        };
        assert_eq!(body.origin, Some((-1, 3)), "{input:?}");
        assert_eq!(body.extent, Some((4, 5)), "{input:?}");
        assert_eq!(body.declared_origin(), (-1, 3), "{input:?}");
        assert_eq!(body.declared_extent(), Some((4, 5)), "{input:?}");

        let mut output = Vec::new();
        serialize_document(&doc, &mut output).unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "glyph foo 2 1 origin -1 3 extent 4 5\n..@@\n",
        );
    }
}

/// `scale 0` is not a scale. It used to parse, multiply the header's `W H` by
/// zero, and turn the pixel rows that followed into unrecognized directives —
/// an error cascade naming everything except the flag that caused it. Every
/// consumer clamps the scale to 1 to stay out of a division by zero; this is
/// what makes those clamps defensive rather than load-bearing.
#[test]
fn a_scale_of_zero_is_rejected_where_it_is_written() {
    let input = "glyph foo 2 2 scale 0\n@@@@\n@@@@\n";
    let err =
        parse_document_from_str(input, "test.unf".into()).expect_err("`scale 0` must not parse");
    assert!(
        format!("{err}").contains("scale"),
        "the error must name the flag, got {err}"
    );
    assert!(
        parse_document_from_str("glyph foo 2 2 scale 1\n@@@@\n@@@@\n", "test.unf".into()).is_ok()
    );
}

/// The one rewriter the box editor uses: it replaces a flag's value where the
/// flag already is, drops a flag whose value is gone, and appends the ones that
/// were never written — leaving the name's quoting, the other flags, the
/// spacing and the trailing comment exactly as they were.
#[test]
fn a_box_flag_is_rewritten_where_it_stands() {
    let cases = [
        // Adding to a header that states nothing.
        (
            "glyph foo 4 2",
            (Some((1, -2)), Some(5), None),
            "glyph foo 4 2 origin 1 -2 advance 5",
        ),
        // Replacing in place, comment and flag order untouched.
        (
            "glyph foo 4 2 advance 5 mark // hi",
            (None, Some(7), None),
            "glyph foo 4 2 advance 7 mark // hi",
        ),
        (
            "glyph 'a b' 4 2 origin 1 -2 keep",
            (Some((3, 0)), None, None),
            "glyph 'a b' 4 2 origin 3 0 keep",
        ),
        // A flag whose value is gone goes with it.
        (
            "glyph foo 4 2 origin 1 -2 mark",
            (None, None, None),
            "glyph foo 4 2 mark",
        ),
        // `extent` replaces `advance`: the two state the same slot, so writing
        // one has to unwrite the other.
        (
            "glyph foo 4 2 advance 5",
            (None, None, Some((5, 16))),
            "glyph foo 4 2 extent 5 16",
        ),
    ];
    for (line, (origin, advance, extent), expected) in cases {
        assert_eq!(
            replace_glyph_box_flags(line, origin, advance, extent).as_deref(),
            Some(expected),
            "{line:?}"
        );
    }

    // Not a glyph header, or an alias: nothing to rewrite.
    assert_eq!(
        replace_glyph_box_flags("ref foo 1 2", None, Some(1), None),
        None
    );
    assert_eq!(
        replace_glyph_box_flags("glyph foo = bar", None, Some(1), None),
        None
    );
}

/// Stating the box's width twice is a mistake, not a precedence question:
/// `advance` and `extent` both say it. Reporting it beats picking a winner,
/// since the source that writes both plainly expects both to count.
#[test]
fn one_header_may_not_state_a_box_slot_twice() {
    let input = "glyph foo 4 2 advance 3 extent 3 2\n@@@@@@@@\n@@@@@@@@\n";
    assert!(
        parse_document_from_str(input, "test.unf".into()).is_err(),
        "{input:?} states the width twice and must not parse"
    );
    // The two flags that state *different* slots are the ordinary case.
    let accepted = "glyph foo 4 2 origin 1 -1 advance 0\n@@@@@@@@\n@@@@@@@@\n";
    let doc = parse_document_from_str(accepted, "test.unf".into()).unwrap();
    let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
        panic!("expected a glyph item");
    };
    // The height is the unstated half, so it reaches from the origin down to
    // the grid's own last row — see `an_unstated_box_dimension_ends_where_the_grid_does`.
    assert_eq!(
        (body.declared_origin(), body.declared_extent()),
        ((1, -1), Some((0, 3)))
    );
}

/// Zero is a declared box's real answer, not a missing one: a combining mark
/// says `extent 0 H` (or, in the old spelling, `advance 0`) to take no width at
/// all. `None` is reserved for a glyph that declares no box — the composite
/// whose box is the union of what it places.
#[test]
fn a_zero_extent_is_declared_and_an_absent_one_is_not() {
    let cases = [
        (
            "glyph foo 4 2 extent 0 0\n@@@@@@@@\n@@@@@@@@\n",
            Some((0, 0)),
        ),
        (
            "glyph foo 4 2 extent 0 9\n@@@@@@@@\n@@@@@@@@\n",
            Some((0, 9)),
        ),
        (
            "glyph foo 4 2 extent 9 0\n@@@@@@@@\n@@@@@@@@\n",
            Some((9, 0)),
        ),
        (
            "glyph foo 4 2 advance 0\n@@@@@@@@\n@@@@@@@@\n",
            Some((0, 2)),
        ),
        ("glyph foo 4 2\n@@@@@@@@\n@@@@@@@@\n", Some((4, 2))),
        ("glyph foo\nref bar\n", None),
    ];
    for (input, expected) in cases {
        let doc = parse_document_from_str(input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {input:?}");
        };
        assert_eq!(body.declared_extent(), expected, "{input:?}");
    }
}

/// An unstated box dimension is the grid's *far edge*, not the grid's size: a
/// glyph that declares an origin has moved the box's corner, and the box still
/// ends where the raster does. `glyph foo 6 16 origin 1 0` advances by 5, not
/// by 6 — the six-wide grid with its first column given away as a bearing.
#[test]
fn an_unstated_box_dimension_ends_where_the_grid_does() {
    let row = "..@@..@@..@@\n";
    let cases = [
        // (header, expected extent)
        ("glyph foo 6 2", Some((6, 2))),
        ("glyph foo 6 2 origin 1 0", Some((5, 2))),
        // A negative origin is a bearing: the box starts left of the grid and
        // still reaches its right edge, so it is *wider* than the grid.
        ("glyph foo 6 2 origin -2 0", Some((8, 2))),
        // The height answers the same way, from the same corner.
        ("glyph foo 6 2 origin 0 1", Some((6, 1))),
        ("glyph foo 6 2 origin 0 -3", Some((6, 5))),
        ("glyph foo 6 2 origin 1 1", Some((5, 1))),
        // An origin past the grid leaves nothing to claim rather than wrapping.
        ("glyph foo 6 2 origin 9 9", Some((0, 0))),
        // What the source states, it states: only the unstated half moves.
        ("glyph foo 6 2 origin 1 0 advance 6", Some((6, 2))),
        ("glyph foo 6 2 origin 1 1 extent 6 2", Some((6, 2))),
    ];
    for (header, expected) in cases {
        let input = format!("{header}\n{row}{row}");
        let doc = parse_document_from_str(&input, "test.unf".into()).unwrap();
        let DocumentItem::Glyph { body, .. } = &doc.items[0] else {
            panic!("expected a glyph item, got {input:?}");
        };
        assert_eq!(body.declared_extent(), expected, "{header:?}");
    }
}

/// An alias is a name for a glyph and nothing else, so the flags a real glyph
/// takes are a mistake on one — silently dropping them would build a font that
/// does not say what the file says.
#[test]
fn strict_parse_rejects_flags_on_an_alias() {
    for input in [
        "glyph foo keep = bar\n",
        "glyph foo advance 5 = bar\n",
        "glyph foo desync = bar\n",
        "glyph foo 2 1 = bar\n",
    ] {
        let err = parse_document_from_str(input, "test.unf".into())
            .expect_err(&format!("should reject: {input:?}"))
            .to_string();
        assert!(err.contains("takes no flags"), "unexpected error: {err}");
    }
}

// -----------------------------------------------------------------------
// Backtick-quoting tokenizer tests
// -----------------------------------------------------------------------
