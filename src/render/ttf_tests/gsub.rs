//! Tests for GSUB generation and the shaping behaviour it produces.

use super::*;

#[test]
fn gsub_tables_generated_for_hangul() {
    let input = "\
font-meta height 16 ascent 12 descent 4

glyph init-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

glyph init-comb-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

glyph med-a 6 8
............
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........

glyph med-comb-a-f 6 8
............
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........

glyph med-comb-a-nof 6 8
............
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........
..@@........

glyph fin-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

glyph fin-comb-g 6 8
@@@@@@@@@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
........@@@@
............

map U+1100 = init-g
map U+1161 = med-a
map U+11A8 = fin-g

remap hangul-ljmo : init-g -> init-comb-g : med-a fin-g
remap hangul-ljmo : init-g -> init-comb-g : med-a
remap hangul-vjmo : med-a -> med-comb-a-f : fin-g
remap hangul-vjmo : med-a -> med-comb-a-nof
remap hangul-tjmo : med-comb-a-f : fin-g -> fin-comb-g
remap hangul-tjmo : med-comb-a-nof : fin-g -> fin-comb-g
feature ljmo for hang : hangul-ljmo
feature vjmo for hang : hangul-vjmo
feature tjmo for hang : hangul-tjmo
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let doc_refs = vec![&doc];

    let (_, _, glyph_data, gsub_data, _) =
        collect_glyph_data(&doc_refs, false).expect("expected glyph data");

    assert!(
        !gsub_data.remap_sets.is_empty(),
        "remap sets should be collected"
    );
    assert!(
        !gsub_data.features.is_empty(),
        "features should be collected"
    );

    assert_eq!(gsub_data.features.len(), 3, "ljmo/vjmo/tjmo features");
    assert_eq!(
        gsub_data.features.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>(),
        ["ljmo", "vjmo", "tjmo"],
    );
    for (_, scripts, _) in &gsub_data.features {
        assert!(
            scripts.contains(&"hang".to_string()),
            "all hangul features should have 'hang' script, got: {scripts:?}"
        );
    }

    let non_cmap_count = glyph_data.iter().filter(|g| g.codepoint.is_none()).count();
    assert!(
        non_cmap_count > 0,
        "remap-referenced non-cmap glyphs should be included"
    );

    let mut name_to_gid: HashMap<String, GlyphId16> = HashMap::new();
    for (i, g) in glyph_data.iter().enumerate() {
        name_to_gid.entry(g.name.clone()).or_insert(GlyphId16::new((i + 1) as u16));
    }
    let gsub = build_gsub(&gsub_data, &name_to_gid);
    assert!(gsub.is_some(), "GSUB table should be generated");
    let gsub = gsub.unwrap();
    let hang_found = gsub.script_list.script_records.iter().any(|r| r.script_tag == Tag::new(b"hang"));
    assert!(hang_found, "GSUB ScriptList should contain 'hang' script");

    let font = build_font_from_documents(&doc_refs);
    assert!(font.is_some(), "font should be built");
}

#[test]
fn gsub_ligature_source_with_spaces() {
    let input = "\
glyph f 1 1
@@
glyph i 1 1
@@
glyph fi 2 1
@@@@
map F = f
map I = i
remap ligset : f i -> fi
feature liga for latn : ligset
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, gsub_data, _) = collect_glyph_data(&[&doc], false).unwrap();

    let mut name_to_gid: HashMap<String, GlyphId16> = HashMap::new();
    for (i, g) in glyph_data.iter().enumerate() {
        name_to_gid.entry(g.name.clone()).or_insert(GlyphId16::new((i + 1) as u16));
    }

    let gsub = build_gsub(&gsub_data, &name_to_gid);
    assert!(gsub.is_some(), "GSUB should be generated for ligature remap");
}

#[test]
fn gsub_feature_script_isolation() {
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
remap set1 : a -> b
remap set2 : c -> d
feature feat for latn : set1
feature feat for arab : set2
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, _glyph_data, gsub_data, _) = collect_glyph_data(&[&doc], false).unwrap();

    assert_eq!(
        gsub_data.features.len(), 2,
        "same tag + different scripts = separate feature entries"
    );
}

#[test]
fn remap_referenced_empty_glyph_survives() {
    let input = "\
glyph a 1 1
@@
glyph b
map A = a
remap set1 : a -> b
feature feat for latn : set1
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    assert!(
        glyph_data.iter().any(|g| g.name == "b"),
        "remap-referenced empty glyph should survive"
    );
}

/// Two `feature` directives sharing a tag *and* a script used to emit two
/// FeatureRecords. A shaper resolves a tag to the first record it finds, so
/// every group after the first was dead — silently, and the more feature
/// tags a font uses up the harder it became to add another remap at all.
#[test]
fn duplicate_feature_tags_merge_into_one_record() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph c = pix
glyph d = pix
map A = a
map C = c
remap first : a -> b
remap second : c -> d
feature ccmp for DFLT : first
feature ccmp for DFLT : second
";
    assert_eq!(
        shape_glyph_names(input, "AC"),
        vec!["b".to_string(), "d".to_string()],
        "both groups under the same tag+script must stay reachable"
    );
}

/// Same tag under *different* scripts stays separate — that is what the
/// script filter is for.
#[test]
fn same_feature_tag_under_different_scripts_stays_separate() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph c = pix
glyph d = pix
map A = a
map C = c
remap first : a -> b
remap second : c -> d
feature ccmp for DFLT : first
feature ccmp for hang : second
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let (_, _, glyph_data, gsub_data, _) = collect_glyph_data(&[&doc], false).unwrap();
    let mut name_to_gid: HashMap<String, GlyphId16> = HashMap::new();
    for (i, g) in glyph_data.iter().enumerate() {
        name_to_gid.entry(g.name.clone()).or_insert(GlyphId16::new((i + 1) as u16));
    }
    let gsub = build_gsub(&gsub_data, &name_to_gid).expect("GSUB");
    assert_eq!(
        gsub.feature_list.feature_records.len(), 2,
        "different scripts must keep their own feature records"
    );
}

/// A group that mixes a multi-glyph source with contextual rules used to be
/// emitted as chain context throughout, and the chain-context helper only
/// ever substituted the *first* input glyph — so `a b -> c` quietly became
/// "replace a, keep b".
#[test]
fn ligature_rule_survives_in_a_contextual_group() {
    let input = "\
glyph pix 1 1
@@
glyph base = pix
glyph t-a = pix
glyph t-b = pix
glyph base-a = pix
glyph cont-b = pix
map U+0042 = base
map U+0061 = t-a
map U+0062 = t-b
map U+0041 = base-a
map U+0051 = cont-b
remap grp : base t-a -> base-a
remap grp : base-a : t-b -> cont-b
feature ccmp for DFLT : grp
";
    assert_eq!(
        shape_glyph_names(input, "Bab"),
        vec!["base-a".to_string(), "cont-b".to_string()],
        "the ligature must ligate even though the group also has context"
    );
}

/// A contextual rule may itself consume several glyphs; the nested lookup
/// has to be a ligature, not a single substitution on the first position.
#[test]
fn contextual_rule_with_a_multi_glyph_source_ligates() {
    let input = "\
glyph pix 1 1
@@
glyph mark = pix
glyph a = pix
glyph b = pix
glyph ab = pix
map U+004D = mark
map U+0061 = a
map U+0062 = b
map U+0058 = ab
remap grp : mark : a b -> ab
feature ccmp for DFLT : grp
";
    assert_eq!(
        shape_glyph_names(input, "Mab"),
        vec!["mark".to_string(), "ab".to_string()],
        "a contextual multi-glyph source must ligate"
    );
    assert_eq!(
        shape_glyph_names(input, "ab"),
        vec!["a".to_string(), "b".to_string()],
        "and must not fire without the context"
    );
}

/// A context-free group mixing single-glyph and multi-glyph sources used to
/// drop the single-glyph rules on the floor.
#[test]
fn single_and_multi_glyph_sources_coexist_in_one_group() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph ab = pix
glyph c = pix
glyph d = pix
map U+0061 = a
map U+0062 = b
map U+0058 = ab
map U+0063 = c
map U+0044 = d
remap grp : a b -> ab
remap grp : c -> d
feature ccmp for DFLT : grp
";
    assert_eq!(
        shape_glyph_names(input, "abc"),
        vec!["ab".to_string(), "d".to_string()],
        "both the ligature and the single substitution must apply"
    );
}

/// One source, several targets — a multiple substitution. Every builder
/// used to keep `target[0]` and drop the rest.
#[test]
fn a_multi_glyph_target_expands() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph c = pix
map U+0061 = a
map U+0042 = b
map U+0043 = c
remap grp : a -> b c
feature ccmp for DFLT : grp
";
    assert_eq!(
        shape_glyph_names(input, "a"),
        vec!["b".to_string(), "c".to_string()],
        "a multi-glyph target must expand"
    );
}

/// An empty target is documented as removal; it used to be skipped by
/// every builder, so the glyph stayed put.
#[test]
fn an_empty_target_removes_the_glyph() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
map U+0061 = a
map U+0062 = b
remap grp : a ->
feature ccmp for DFLT : grp
";
    assert_eq!(
        shape_glyph_names(input, "ab"),
        vec!["b".to_string()],
        "an empty target must delete the glyph"
    );
}

/// `feature ... for latn/ROM` must reach Romanian and nothing else.
///
/// The shape of the bug this pins is a font-wide `locl`: declaring the feature
/// for the bare script would turn Turkish `ş` into `ș` too, which is exactly
/// the substitution a language system exists to avoid.
#[test]
fn a_language_system_only_applies_to_its_own_language() {
    let input = "\
glyph pix 1 1
@@
glyph s-cedilla = pix
glyph s-comma = pix
map U+015F = s-cedilla
map U+0219 = s-comma
remap romanian : s-cedilla -> s-comma
feature locl for latn/ROM : romanian
";
    assert_eq!(
        shape_glyph_names_in(input, "\u{15f}", Some("ro")),
        vec!["s-comma".to_string()],
        "Romanian must get the comma-below glyph"
    );
    assert_eq!(
        shape_glyph_names_in(input, "\u{15f}", Some("tr")),
        vec!["s-cedilla".to_string()],
        "Turkish must keep the cedilla"
    );
    assert_eq!(
        shape_glyph_names_in(input, "\u{15f}", None),
        vec!["s-cedilla".to_string()],
        "the default language system must keep the cedilla"
    );
}

/// A shaper picks an explicit LangSys *instead of* the default one, so
/// everything the default carries has to be repeated in it. Left out, adding a
/// single `locl` for Romanian would silently disable `ccmp` — and with it every
/// mark attachment — for Romanian alone.
#[test]
fn a_language_system_inherits_the_default_features() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph s-cedilla = pix
glyph s-comma = pix
map U+0061 = a
map U+0062 = b
map U+015F = s-cedilla
map U+0219 = s-comma
remap always : a -> b
remap romanian : s-cedilla -> s-comma
feature ccmp for latn : always
feature locl for latn/ROM : romanian
";
    assert_eq!(
        shape_glyph_names_in(input, "a", Some("ro")),
        vec!["b".to_string()],
        "the script-wide ccmp must still apply under an explicit language"
    );
    assert_eq!(
        shape_glyph_names_in(input, "a\u{15f}", Some("ro")),
        vec!["b".to_string(), "s-comma".to_string()],
        "both the inherited and the language's own feature must apply"
    );
}

/// The inheritance merges per feature *tag*: a language redefining a tag the
/// default already declares must end up with one record holding both sets of
/// lookups, not two records the shaper would resolve to whichever came first.
#[test]
fn a_language_redefining_a_tag_merges_with_the_default() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph s-cedilla = pix
glyph s-comma = pix
map U+0061 = a
map U+0062 = b
map U+015F = s-cedilla
map U+0219 = s-comma
remap always : a -> b
remap romanian : s-cedilla -> s-comma
feature ccmp for latn : always
feature ccmp for latn/ROM : romanian
";
    assert_eq!(
        shape_glyph_names_in(input, "a\u{15f}", Some("ro")),
        vec!["b".to_string(), "s-comma".to_string()],
        "both lookups must live under the one ccmp record Romanian resolves to"
    );
    assert_eq!(
        shape_glyph_names_in(input, "a\u{15f}", Some("tr")),
        vec!["b".to_string(), "s-cedilla".to_string()],
        "the default record must be untouched by the language's addition"
    );
}

/// `DFLT` is a fallback, not a wildcard: a shaper consults it only when the
/// script it wants has no record of its own. Declaring one feature for a real
/// script therefore used to make that script blind to everything under DFLT —
/// adding a Romanian `locl` cost all Latin text its `ccmp`, and every mark
/// attachment with it.
#[test]
fn declaring_a_script_does_not_hide_the_default_features() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph s-cedilla = pix
glyph s-comma = pix
map U+0061 = a
map U+0062 = b
map U+015F = s-cedilla
map U+0219 = s-comma
remap always : a -> b
remap romanian : s-cedilla -> s-comma
feature ccmp for DFLT : always
feature locl for latn/ROM : romanian
";
    assert_eq!(
        shape_glyph_names_in(input, "a", None),
        vec!["b".to_string()],
        "Latin text must keep the DFLT ccmp even though latn is now declared"
    );
    assert_eq!(
        shape_glyph_names_in(input, "a\u{15f}", Some("ro")),
        vec!["b".to_string(), "s-comma".to_string()],
        "Romanian must see the DFLT ccmp and its own locl"
    );
}

/// `usMaxContext` is the longest sequence a lookup can match, and for a
/// chaining lookup the spec counts the backtrack too. Leaving lookbehind out
/// under-reports it, which is exactly what a client that pre-buffers
/// `usMaxContext` glyphs around an edit would get wrong.
#[test]
fn max_context_counts_the_lookbehind() {
    fn max_context_of(input: &str) -> u16 {
        let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
        let built = build_font_with_gid_map(&[&doc]).expect("font should build");
        let font = read_fonts::FontRef::new(&built.ttf).unwrap();
        font.os2().unwrap().us_max_context().unwrap()
    }

    let head = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph c = pix
glyph d = pix
glyph e = pix
map U+0061 = a
map U+0062 = b
map U+0063 = c
map U+0064 = d
map U+0065 = e
";

    assert_eq!(
        max_context_of(&format!("{head}remap grp : a b -> c\nfeature ccmp for DFLT : grp\n")),
        2,
        "a plain ligature matches its own source and nothing more"
    );
    assert_eq!(
        max_context_of(&format!("{head}remap grp : a : b -> c\nfeature ccmp for DFLT : grp\n")),
        2,
        "one glyph of lookbehind plus a one-glyph source"
    );
    assert_eq!(
        max_context_of(&format!(
            "{head}remap grp : a b : c -> d : e\nfeature ccmp for DFLT : grp\n"
        )),
        4,
        "two of lookbehind, one source, one of lookahead"
    );
}

/// Lookup index order is application order, and it belongs to the groups: the
/// pass that runs first is the group whose rules come first, whatever order the
/// `feature` lines that attach them happen to be in.
#[test]
fn group_order_and_not_feature_order_decides_which_pass_runs_first() {
    let head = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
glyph c = pix
map U+0061 = a
map U+0062 = b
map U+0063 = c
";
    // `first` turns a into b, `second` turns b into c. Chaining a all the way
    // to c can only happen if `first` runs first.
    let rules = "remap first : a -> b\nremap second : b -> c\n";
    let features_reversed = "\
feature ccmp for DFLT : second
feature ccmp for DFLT : first
";
    assert_eq!(
        shape_glyph_names(&format!("{head}{rules}{features_reversed}"), "a"),
        vec!["c".to_string()],
        "the rules put `first` first; the feature lines must not override that"
    );

    // And `after` is what actually moves a group.
    let moved = "remap group first after second\n";
    assert_eq!(
        shape_glyph_names(&format!("{head}{rules}{moved}{features_reversed}"), "a"),
        vec!["b".to_string()],
        "with `first` moved after `second`, nothing is left to turn b into c"
    );
}

/// A `reversed` group is not built yet, but declaring one must not silently
/// change what the group does in the meantime.
#[test]
fn a_bare_group_declaration_changes_nothing() {
    let input = "\
glyph pix 1 1
@@
glyph a = pix
glyph b = pix
map U+0061 = a
map U+0062 = b
remap grp : a -> b
remap group grp
feature ccmp for DFLT : grp
";
    assert_eq!(shape_glyph_names(input, "a"), vec!["b".to_string()]);
}

/// The point of a `reversed` group: it runs right to left, so its lookahead
/// matches what the same lookup has *already* produced and the rule repeats
/// leftward over a run of any length. The forward spelling of this needs one
/// rule per run length.
#[test]
fn a_reversed_group_chains_leftward_without_a_bound() {
    let input = "\
glyph pix 1 1
@@
glyph eq = pix
glyph gt = pix
glyph eq-gt = pix
glyph eq-cont = pix
map U+003D = eq
map U+003E = gt
map U+0051 = eq-gt
map U+0052 = eq-cont
remap liga : eq gt -> eq-gt
remap cont : eq -> eq-cont : eq-cont
remap cont : eq -> eq-cont : eq-gt
remap group cont reversed after liga
feature calt for DFLT : liga
feature calt for DFLT : cont
";
    for run in 1..=12 {
        let text = format!("{}>", "=".repeat(run));
        let mut expected = vec!["eq-cont".to_string(); run - 1];
        expected.push("eq-gt".to_string());
        assert_eq!(shape_glyph_names(input, &text), expected, "run of {run}");
    }

    // Without the ligature ahead of them, a run of `=` is left alone.
    assert_eq!(
        shape_glyph_names(input, "==="),
        vec!["eq".to_string(), "eq".to_string(), "eq".to_string()],
    );
}
