//! Tests of anchor exposure, derivation reports, and minus/plus attachment.

use super::*;

/// Anchor inheritance is opt-in: a composite exposes only its own declared
/// anchors plus the surviving anchors of refs marked `inherit`. Attachment
/// *inside* the composite works regardless of the flag.
#[test]
fn anchor_exposure_requires_inherit() {
    let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
anchor +below 1 3
glyph mark 2 1 mark
@@@@
anchor -above 0 0
anchor +above 0 -1
glyph opaque
ref base
ref mark
glyph transparent
ref base inherit
ref mark inherit
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    // The mark attached in both composites (its -above consumed base's +above).
    for name in ["opaque", "transparent"] {
        let g = &resolved[name];
        assert!(
            !g.resolved_anchors.iter().any(|p| p.position == "-above"),
            "{name}: consumed -above must not be exposed: {:?}",
            g.resolved_anchors,
        );
    }

    let opaque = &resolved["opaque"];
    assert!(
        opaque.resolved_anchors.is_empty(),
        "no inherit, no declared anchors: nothing exposed, got {:?}",
        opaque.resolved_anchors,
    );

    let transparent = &resolved["transparent"];
    let positions: Vec<&str> = transparent
        .resolved_anchors
        .iter()
        .map(|p| p.position.as_str())
        .collect();
    assert!(
        positions.contains(&"+below"),
        "base's +below survives: {positions:?}"
    );
    assert!(
        positions.contains(&"+above"),
        "mark's republished +above survives: {positions:?}"
    );
    let above = transparent
        .resolved_anchors
        .iter()
        .find(|p| p.position == "+above")
        .unwrap();
    // mark's own +above (0, -1) translated by the attachment offset (1, 0).
    assert_eq!(
        (above.col, above.row),
        (1, -1),
        "the surviving +above is the mark's, moved"
    );
}

/// Two inherit refs surviving with the same anchor name is an error, and the
/// fallback acts as if that anchor did not exist at all — a digraph must not
/// pick one side's attachment point silently.
#[test]
fn duplicate_exposed_anchors_are_dropped() {
    let input = "\
glyph half 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
anchor +below 1 3
glyph digraph
ref half 0 0 inherit
ref half 4 0 inherit
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let digraph = &resolved["digraph"];
    assert!(
        digraph.resolved_anchors.is_empty(),
        "both +above and both +below collide; all must be dropped, got {:?}",
        digraph.resolved_anchors,
    );
}

/// A minus anchor no remaining ref can ever satisfy must not defer its ref:
/// deferral would let explicit-offset sibling refs commit first, miss their
/// consumption, and leave the base's occupied anchor exposed.
#[test]
fn unsatisfiable_minus_does_not_defer_commit_order() {
    let input = "\
glyph base 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor -center 1 1
anchor +below 1 3
glyph dot 1 1 mark
@@
anchor -below 0 0
anchor +below 0 1
glyph comp
ref base inherit
ref dot 1 3
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let comp = &resolved["comp"];
    let positions: Vec<&str> = comp
        .resolved_anchors
        .iter()
        .map(|p| p.position.as_str())
        .collect();
    assert!(
        positions.contains(&"-center"),
        "base's unsatisfiable -center is forwarded through inherit: {positions:?}"
    );
    assert!(
        !positions.contains(&"+below"),
        "base must commit before the explicit-offset dot so the dot consumes \
         +below; it must not linger exposed: {positions:?}"
    );
}

/// `map generate` composites stand in for their decomposition, so their
/// synthesized refs inherit implicitly: the generated glyph exposes the
/// surviving anchors exactly as the hand-written equivalent with `inherit`.
#[test]
fn map_generate_refs_inherit_implicitly() {
    let input = "\
glyph a-upper 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1 0
anchor +below 1 3
glyph grave 2 1 mark
@@@@
anchor -above 0 0
anchor +above 0 -1
map A = a-upper
map U+0300 = grave
map generate \u{c0}
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let generated = resolved.get("uni00C0").expect("generated composite");
    let positions: Vec<&str> = generated
        .resolved_anchors
        .iter()
        .map(|p| p.position.as_str())
        .collect();
    assert!(positions.contains(&"+above"), "{positions:?}");
    assert!(positions.contains(&"+below"), "{positions:?}");
    assert!(!positions.contains(&"-above"), "{positions:?}");
}

/// The derive-level diagnostics carry which anchor collided and which ref
/// attached ambiguously, so issues.rs can report them per glyph.
#[test]
fn derive_reports_duplicates_and_ambiguity() {
    let anchored = |position: &str, col: i16, row: i16| GlyphPoint {
        comment: None,
        scale: 1,
        position: position.to_string(),
        col,
        row,
        col_end: col,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            "half" => Some(vec![anchored("+above", 1, 0)]),
            "mark" => Some(vec![anchored("-above", 0, 0)]),
            _ => None,
        }
    };
    let inherit_ref = |name: &str, col: i16| GlyphRef {
        raw_name: None,
        comment: None,
        name: name.to_string(),
        offset: Some((col, 0)),
        negated: false,
        inherit: true,
        goto: false,
        fill: None,
        visibility: None,
    };

    // Two inherited +above survive → both dropped, one issue.
    let refs = vec![inherit_ref("half", 0), inherit_ref("half", 4)];
    let (_, exposed, issues) = derive_ref_offsets_with(
        &[],
        &refs,
        1,
        &AnchorAligns::default(),
        lookup,
        |_| Vec::new(),
        lookup,
        |_: &str| (0, 0),
    );
    assert!(exposed.is_empty(), "{exposed:?}");
    assert_eq!(
        issues,
        vec![DeriveIssue::DuplicateExposed {
            position: "+above".into()
        }],
    );

    // A mark whose -above finds two +above candidates attaches to neither.
    let refs = vec![
        inherit_ref("half", 0),
        inherit_ref("half", 4),
        GlyphRef {
            raw_name: None,
            offset: None,
            inherit: false,
            ..inherit_ref("mark", 0)
        },
    ];
    let (effective, _, issues) = derive_ref_offsets_with(
        &[],
        &refs,
        1,
        &AnchorAligns::default(),
        lookup,
        |_| Vec::new(),
        lookup,
        |_: &str| (0, 0),
    );
    assert_eq!(effective[2].offset, Some((0, 0)), "unattached fallback");
    assert!(
        issues.contains(&DeriveIssue::AmbiguousAttachment {
            position: "-above".into(),
            ref_name: "mark".into(),
        }),
        "{issues:?}",
    );
}

/// Manual migration helper: compares the current opt-in anchor exposure over
/// `font/` against forward-everything (every ref forced `inherit`), listing
/// the glyphs whose `+above`/`+below` disappeared.
/// `cargo test -r probe_migration_worklist -- --ignored --nocapture`
#[test]
#[ignore]
fn probe_migration_worklist() {
    let docs =
        crate::render::ttf_builder::load_docs_from_directory_checked(std::path::Path::new("font"))
            .0;
    let refs: Vec<&Document> = docs.iter().collect();
    let name_parts = crate::document::collect_name_parts(&refs);
    let (resolved, _alt) = resolve_named_glyphs_with_parts(&refs, &name_parts);

    // The same font with every ref forced to inherit approximates the old
    // forward-everything behavior.
    let mut all_inherit: Vec<Document> = docs.clone();
    for doc in &mut all_inherit {
        for item in &mut doc.items {
            if let DocumentItem::Glyph { body, .. } = item {
                for r in &mut body.refs {
                    r.inherit = true;
                }
            }
        }
    }
    let refs2: Vec<&Document> = all_inherit.iter().collect();
    let (resolved_old, _alt2) = resolve_named_glyphs_with_parts(&refs2, &name_parts);

    for pos in ["-above", "-below", "+above", "+below"] {
        let now = resolved
            .values()
            .filter(|g| g.resolved_anchors.iter().any(|p| p.position == pos))
            .count();
        let old = resolved_old
            .values()
            .filter(|g| g.resolved_anchors.iter().any(|p| p.position == pos))
            .count();
        eprintln!("{pos}: exposed by {now} glyphs (forward-everything: {old})");
    }

    let mut lost: Vec<&str> = Vec::new();
    for (name, old) in &resolved_old {
        let Some(new) = resolved.get(name) else {
            continue;
        };
        for pos in ["+above", "+below"] {
            let had = old.resolved_anchors.iter().any(|p| p.position == pos);
            let has = new.resolved_anchors.iter().any(|p| p.position == pos)
                || new.declared_anchors.iter().any(|p| p.position == pos);
            if had && !has {
                lost.push(name);
                break;
            }
        }
    }
    lost.sort();
    lost.dedup();
    eprintln!(
        "== {} glyphs no longer expose a +above/+below they used to:",
        lost.len()
    );
    for n in &lost {
        eprintln!("  {n}");
    }
}

/// A `+` anchor whose range is *bigger* than the `-` asked of it holds that
/// mark, exactly as GPOS's `slot_holds` puts a mark in a base's slot, and the
/// mark reduces by the class's `align` on the way in. The two paths have to
/// agree here: `he-yod-with-hiriq` precomposes what `he-yod` + `he-hiriq`
/// shapes to, and demanding an exact size dropped the precomposed glyph while
/// the shaped pair attached happily.
#[test]
fn a_wider_plus_holds_a_narrower_minus() {
    let anchored = |position: &str, col: i16, col_end: i16, row: i16| GlyphPoint {
        comment: None,
        scale: 1,
        position: position.to_string(),
        col,
        row,
        col_end,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            // `he-yod`'s hosting range: the whole width of the letter.
            "base" => Some(vec![anchored("+below", 0, 6, 13)]),
            // `he-hiriq`'s footprint: three cells wide, drawn at 3..5.
            "mark" => Some(vec![anchored("-below", 3, 5, 0)]),
            _ => None,
        }
    };
    let gref = |name: &str| GlyphRef {
        raw_name: None,
        comment: None,
        name: name.to_string(),
        offset: None,
        negated: false,
        inherit: false,
        goto: false,
        fill: None,
        visibility: None,
    };
    let refs = vec![gref("base"), gref("mark")];

    let mut aligns = AnchorAligns::default();
    aligns.insert(
        "below".to_string(),
        AnchorAlign {
            vertical: Align1::Center,
            horizontal: Align1::Center,
        },
    );
    let (effective, _, issues) = derive_ref_offsets_with(
        &[],
        &refs,
        1,
        &aligns,
        lookup,
        |_| Vec::new(),
        lookup,
        |_: &str| (0, 0),
    );
    assert!(issues.is_empty(), "{issues:?}");
    // Centre of 0..6 is 3, centre of 3..5 is 4: the mark comes one cell left.
    assert_eq!(effective[1].offset, Some((-1, 13)), "{effective:?}");

    // The default reduction is the low end of each axis, and it places the
    // same pair from there instead.
    let (effective, issues) = {
        let (e, _, i) = derive_ref_offsets_with(
            &[],
            &refs,
            1,
            &AnchorAligns::default(),
            lookup,
            |_| Vec::new(),
            lookup,
            |_: &str| (0, 0),
        );
        (e, i)
    };
    assert!(issues.is_empty(), "{issues:?}");
    assert_eq!(effective[1].offset, Some((-3, 13)), "{effective:?}");
}

/// A `-` anchor bigger than every same-name `+` it finds attaches to nothing,
/// and that near-miss is reported: it almost always means the wrong
/// `:narrow`/`:wide` variant was picked. A minus with no same-name `+` at all
/// stays quiet — that is ordinary alias forwarding. (A `+` merely *larger*
/// than the `-` is no near-miss at all: it holds the mark, exactly as a base
/// slot holds a mark in GPOS.)
#[test]
fn derive_reports_size_mismatched_attachment() {
    let anchored = |position: &str, col: i16, row: i16, w: i16| GlyphPoint {
        comment: None,
        scale: 1,
        position: position.to_string(),
        col,
        row,
        col_end: col + w - 1,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            "base" => Some(vec![anchored("+above", 1, 0, 1)]),
            "mark" => Some(vec![anchored("-above", 0, 0, 2)]),
            _ => None,
        }
    };
    let gref = |name: &str, offset: Option<(i16, i16)>| GlyphRef {
        raw_name: None,
        comment: None,
        name: name.to_string(),
        offset,
        negated: false,
        inherit: false,
        goto: false,
        fill: None,
        visibility: None,
    };

    // Explicit offset, and a mark the one-cell +above could not hold anyway.
    let refs = vec![gref("base", None), gref("mark", Some((1, 2)))];
    let (_, _, issues) = derive_ref_offsets_with(
        &[],
        &refs,
        1,
        &AnchorAligns::default(),
        lookup,
        |_| Vec::new(),
        lookup,
        |_: &str| (0, 0),
    );
    assert!(
        issues.contains(&DeriveIssue::SizeMismatchedAttachment {
            position: "-above".into(),
            ref_name: "mark".into(),
            minus: (2, 1),
            plus: (1, 1),
        }),
        "{issues:?}",
    );

    // No same-name + anywhere: plain forwarding, no warning.
    let refs = vec![gref("mark", None)];
    let (_, _, issues) = derive_ref_offsets_with(
        &[],
        &refs,
        1,
        &AnchorAligns::default(),
        lookup,
        |_| Vec::new(),
        lookup,
        |_: &str| (0, 0),
    );
    assert!(issues.is_empty(), "{issues:?}");
}

/// Several `-` anchors on one ref target are *alternatives*, not several
/// attachments: one combining mark that can adjoin to more than one anchor
/// system. Attaching through one retires the rest — they neither survive as
/// exposed anchors nor warn about a same-name `+` of another size — and their
/// `+` partners go with them, so the mark publishes only the system it
/// actually joined.
///
/// `gr-psili` over a Greek capital is the case: it declares `-gr-above` and
/// forwards `-above` from `com-above:narrow`, while the capital publishes a
/// 1-cell `+gr-above` and a 2-cell `+above`. It joins the Greek system; the
/// leftover 1-cell `-above` used to survive, warn against the capital's
/// 2-cell `+above`, and publish a second `+above` that collided with it.
#[test]
fn attaching_through_one_minus_retires_the_other_alternatives() {
    let anchored = |position: &str, col: i16, row: i16, w: i16| GlyphPoint {
        comment: None,
        scale: 1,
        position: position.to_string(),
        col,
        row,
        col_end: col + w - 1,
        row_end: row,
    };
    let lookup = |name: &str| -> Option<Vec<GlyphPoint>> {
        match name {
            "cap" => Some(vec![
                anchored("+above", 3, 1, 2),
                anchored("+gr-above", 0, 3, 1),
            ]),
            "psili" => Some(vec![
                anchored("-gr-above", 1, 1, 1),
                anchored("+gr-above", 1, -1, 1),
                anchored("-above", 1, 2, 1),
                anchored("+above", 1, -1, 1),
            ]),
            _ => None,
        }
    };
    let gref = |name: &str| GlyphRef {
        raw_name: None,
        comment: None,
        name: name.to_string(),
        offset: None,
        negated: false,
        inherit: true,
        goto: false,
        fill: None,
        visibility: None,
    };

    let refs = vec![gref("cap"), gref("psili")];
    let (effective, exposed, issues) = derive_ref_offsets_with(
        &[],
        &refs,
        1,
        &AnchorAligns::default(),
        lookup,
        |_| Vec::new(),
        lookup,
        |_: &str| (0, 0),
    );

    // Joined through -gr-above: offset = plus(0,3) - minus(1,1).
    assert_eq!(effective[1].offset, Some((-1, 2)));
    assert!(issues.is_empty(), "{issues:?}");

    let mut positions: Vec<&str> = exposed.iter().map(|(p, _)| p.position.as_str()).collect();
    positions.sort_unstable();
    // The capital's untouched +above, and the Greek system psili published.
    // Not psili's -above (retired), and so not a second +above either.
    assert_eq!(positions, vec!["+above", "+gr-above"]);
}

/// Size-based alternative selection still runs for offset-less refs, and an
/// exact fit is what it goes by: the uni1E2E shape (a narrow mark stacked on a
/// wide mark's 2-cell `+above`) picks the `:wide` alternative, whose `-above`
/// is that slot's own size, over the narrow one the slot would merely hold.
/// The same refs pinned by explicit offsets cannot substitute; the narrow mark
/// is then held by the wider slot, as GPOS would hold it, and nothing is
/// reported — a `-` no `+` is big enough for is the near-miss, and
/// `derive_reports_size_mismatched_attachment` is where that lives.
#[test]
fn offsetless_stacked_mark_picks_wide_alternative_without_warning() {
    let input = "\
glyph i-compressed 4 4
@@@@@@@@
@@@@@@@@
@@@@@@@@
@@@@@@@@
anchor +above 1..2 0

glyph dia 4 1 mark
@@@@@@@@
anchor -above 1..2 0
anchor +above 1..2 -1

glyph acute 2 1 mark
@@@@
anchor -above 0 0
anchor +above 0 -1

glyph acute:wide 2 1 mark
@@@@
anchor -above 0..1 0
anchor +above 0..1 -1

glyph stacked
ref i-compressed
ref dia
ref acute
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let body = doc
        .items
        .iter()
        .find_map(|item| match item {
            DocumentItem::Glyph { name, body } if name.display() == "stacked" => Some(body),
            _ => None,
        })
        .unwrap();
    let derive = |refs: &[GlyphRef]| {
        derive_ref_offsets_with(
            &body.points,
            refs,
            1,
            &AnchorAligns::default(),
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
            |_: &str| (0, 0),
        )
    };

    // Offset-less: the narrow acute cannot consume dia's 2-cell +above, so
    // the 2-cell `acute:wide` is substituted; everything attaches, no issue.
    let (effective, _, issues) = derive(&body.refs);
    assert_eq!(effective[2].name, "acute:wide");
    assert!(issues.is_empty(), "{issues:?}");

    // The same refs pinned by explicit offsets: no substitution is possible,
    // and the narrow acute is held by the two-cell slot as it stands.
    let pinned: Vec<GlyphRef> = body
        .refs
        .iter()
        .enumerate()
        .map(|(i, r)| GlyphRef {
            raw_name: None,
            offset: Some((0, i as i16)),
            ..r.clone()
        })
        .collect();
    let (effective, _, issues) = derive(&pinned);
    assert_eq!(
        effective[2].name, "acute",
        "explicit offsets never substitute"
    );
    assert!(issues.is_empty(), "{issues:?}");
}

/// Alternative selection also runs on the *publisher* side, by size: an
/// offset-less ref whose declared `+X` name-matches but size-mismatches a
/// sibling consumer's `-X` is substituted by an alternative whose `+X` fits.
/// This is the `enclosing-circle:alt` case — the letters cannot adapt (there
/// is no descender variant with a taller `-center`), so the circle must.
#[test]
fn publisher_alternative_is_selected_by_anchor_size() {
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

glyph a-inner 2 2
@@@@
@@@@
anchor -center 1 0..1

glyph j-inner 2 2
@@@@
@@@@
anchor -center 1 0

glyph a-circled
ref circle
ref a-inner

glyph j-circled
ref circle
ref j-inner

glyph j-circled-reversed
ref j-inner
ref circle
";
    let doc = crate::document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let docs = [&doc];
    let name_parts = crate::document::collect_name_parts(&docs);
    let (resolved, alt_idx) = resolve_named_glyphs_with_parts(&docs, &name_parts);

    let derive = |glyph: &str| {
        let body = doc
            .items
            .iter()
            .find_map(|item| match item {
                DocumentItem::Glyph { name, body } if name.display() == glyph => Some(body),
                _ => None,
            })
            .unwrap();
        derive_ref_offsets_with(
            &body.points,
            &body.refs,
            1,
            &AnchorAligns::default(),
            |name| resolved.get(name).map(|r| r.resolved_anchors.clone()),
            |name| alt_idx.get(name).to_vec(),
            |name| resolved.get(name).map(|r| r.declared_anchors.clone()),
            |_: &str| (0, 0),
        )
    };

    // The 2-cell consumer matches the primary circle: no substitution.
    let (effective, _, issues) = derive("a-circled");
    assert_eq!(effective[0].name, "circle");
    assert!(issues.is_empty(), "{issues:?}");

    // The 1-cell consumer fits only circle:alt, whichever side comes first.
    for glyph in ["j-circled", "j-circled-reversed"] {
        let (effective, _, issues) = derive(glyph);
        let circle_ref = effective
            .iter()
            .find(|r| r.name.starts_with("circle"))
            .unwrap();
        assert_eq!(circle_ref.name, "circle:alt", "{glyph}");
        assert!(issues.is_empty(), "{glyph}: {issues:?}");
        // The consumer really attached: its offset aligns -center on (2, 1).
        let inner = effective
            .iter()
            .find(|r| r.name.ends_with("-inner"))
            .unwrap();
        assert_eq!(inner.offset, Some((1, 1)), "{glyph}");
    }
}
