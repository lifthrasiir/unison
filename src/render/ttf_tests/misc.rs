//! Assorted builder tests: the build digest, `meta` handling, `map`
//! expansion helpers and the `maxp` limits.

use super::*;

#[test]
fn ttf_build_digest_is_deterministic() {
    let input = "\
meta height 16
meta ascent 12
meta descent 4

glyph base 2 2
@@@@
..@@

glyph wide 3 2
@@..@@
..@@@@

glyph comp
ref base
ref wide

glyph alias = base

map A = base
map B = wide
map C = comp
map D = alias
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let doc_refs = vec![&doc];

    let (_, _, glyph_data_a, _, _) =
        collect_glyph_data(&doc_refs, false).expect("expected glyph data");
    let (_, _, glyph_data_b, _, _) =
        collect_glyph_data(&doc_refs, false).expect("expected glyph data");

    let mut canon_a: Vec<_> = glyph_data_a.iter().map(canonicalize_glyph).collect();
    let mut canon_b: Vec<_> = glyph_data_b.iter().map(canonicalize_glyph).collect();
    canon_a.sort();
    canon_b.sort();
    assert_eq!(
        canon_a, canon_b,
        "canonicalized glyph data should be deterministic"
    );
    assert!(!canon_a.is_empty(), "should produce glyphs");
}

/// Read every platform-3 name record out of a built font as
/// `(name ID, language ID, string)`.
fn name_records(bytes: &[u8]) -> Vec<(u16, u16, String)> {
    let font = read_fonts::FontRef::new(bytes).unwrap();
    let name = font.name().unwrap();
    name.name_record()
        .iter()
        .map(|rec| {
            let s = rec
                .string(name.string_data())
                .unwrap()
                .chars()
                .collect::<String>();
            (rec.name_id.get().to_u16(), rec.language_id(), s)
        })
        .collect()
}

fn build_from(src: &str) -> Vec<u8> {
    let doc = document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    build_font_from_documents(&[&doc]).expect("expected a font")
}

/// The name table is what the OS files the font under, so it has to come from
/// the source rather than from a constant in the builder — a hardcoded family
/// name is a font that installs under the wrong name.
#[test]
fn declared_metadata_reaches_the_name_table() {
    let ttf = build_from(
        "meta family `Unison`\n\
         meta subfamily `Bold`\n\
         meta revision 1.25\n\
         meta vendor-id UNSN\n\
         meta copyright `May you do good and not evil.`\n\
         meta license-url `https://example.invalid/license`\n\
         glyph a 1 1\n@@\nmap A = a\n",
    );
    let recs = name_records(&ttf);
    let get = |id: u16| {
        recs.iter()
            .find(|&&(i, lang, _)| i == id && lang == 0x0409)
            .map(|(_, _, s)| s.as_str())
    };
    assert_eq!(get(0), Some("May you do good and not evil."));
    assert_eq!(get(1), Some("Unison"));
    assert_eq!(get(2), Some("Bold"));
    assert_eq!(get(14), Some("https://example.invalid/license"));
    // Derived, not declared.
    assert_eq!(get(4), Some("Unison Bold"));
    assert_eq!(get(5), Some("Version 1.250"));
    assert_eq!(get(6), Some("Unison-Bold"));
    assert_eq!(get(3), Some("Version 1.250;UNSN;Unison-Bold"));

    let font = read_fonts::FontRef::new(&ttf).unwrap();
    assert_eq!(font.head().unwrap().font_revision().to_f32(), 1.25);
    assert_eq!(
        font.os2().unwrap().ach_vend_id(),
        read_fonts::types::Tag::new(b"UNSN")
    );
}

/// A localized record is filed under its own language ID and sits alongside
/// the en-US one rather than replacing it.
#[test]
fn a_localized_family_becomes_its_own_name_record() {
    let ttf = build_from(
        "meta family `Unison`\nmeta family @ko-KR `유니슨`\nglyph a 1 1\n@@\nmap A = a\n",
    );
    let recs = name_records(&ttf);
    assert!(
        recs.contains(&(1, 0x0409, "Unison".to_string())),
        "got {recs:?}"
    );
    assert!(
        recs.contains(&(1, 0x0412, "유니슨".to_string())),
        "got {recs:?}"
    );
}

/// `ulUnicodeRange` advertises what the font covers; Windows font fallback and
/// font pickers read it. Leaving it zero says "covers nothing", which is what
/// the builder used to emit.
#[test]
fn unicode_ranges_are_derived_from_the_cmap() {
    let ttf = build_from(
        "glyph a 1 1\n@@\n\
         map A = a\nmap Γ = a\nmap Б = a\nmap 中 = a\n",
    );
    let os2 = read_fonts::FontRef::new(&ttf).unwrap().os2().unwrap();
    let bits = [
        os2.ul_unicode_range_1(),
        os2.ul_unicode_range_2(),
        os2.ul_unicode_range_3(),
        os2.ul_unicode_range_4(),
    ];
    let is_set = |bit: u32| bits[(bit / 32) as usize] & (1 << (bit % 32)) != 0;
    assert!(is_set(0), "Basic Latin");
    assert!(is_set(7), "Greek and Coptic");
    assert!(is_set(9), "Cyrillic");
    assert!(is_set(59), "CJK Unified Ideographs");
    assert!(!is_set(56), "Hangul Syllables is not covered here");
    assert!(!is_set(57), "no supplementary-plane character here");
}

/// Bit 57 (Non-Plane 0) means "at least one character beyond the BMP". The
/// generated block table only carries the surrogate range D800..DFFF for it,
/// which no Unicode cmap ever maps — a supplementary codepoint has to set the
/// bit directly.
#[test]
fn supplementary_codepoint_sets_non_plane_0_bit() {
    let ttf = build_from("glyph a 1 1\n@@\nmap U+1F1E6 = a\n");
    let os2 = read_fonts::FontRef::new(&ttf).unwrap().os2().unwrap();
    assert_ne!(
        os2.ul_unicode_range_2() & (1 << (57 - 32)),
        0,
        "U+1F1E6 is beyond the BMP, so Non-Plane 0 must be set"
    );
}

/// `ulCodePageRange` used to be hardcoded to Latin-1 regardless of coverage.
#[test]
fn code_page_ranges_are_derived_from_the_cmap() {
    // Latin 1 needs printable ASCII plus its marker character.
    let mut src = String::from("glyph a 1 1\n@@\n");
    for cp in 0x20u32..=0x7E {
        src.push_str(&format!("map U+{cp:04X} = a\n"));
    }
    src.push_str("map Þ = a\n");
    let ttf = build_from(&src);
    let os2 = read_fonts::FontRef::new(&ttf).unwrap().os2().unwrap();
    assert_eq!(os2.ul_code_page_range_1().unwrap() & 1, 1, "Latin 1");

    // Without the ASCII range there is no Latin 1 claim to make.
    let ttf = build_from("glyph a 1 1\n@@\nmap Þ = a\n");
    let os2 = read_fonts::FontRef::new(&ttf).unwrap().os2().unwrap();
    assert_eq!(
        os2.ul_code_page_range_1().unwrap() & 1,
        0,
        "no ASCII, no Latin 1"
    );
}

/// `post.isFixedPitch` was hardcoded to 1 in a font whose advances are not all
/// equal. It is a claim about the font, so it comes from `meta`.
#[test]
fn fixed_pitch_is_declared_rather_than_assumed() {
    let ttf = build_from("glyph a 1 1\n@@\nmap A = a\n");
    let post = read_fonts::FontRef::new(&ttf).unwrap().post().unwrap();
    assert_eq!(post.is_fixed_pitch(), 0, "not claimed unless declared");

    let ttf = build_from("meta fixed-pitch\nglyph a 1 1\n@@\nmap A = a\n");
    let post = read_fonts::FontRef::new(&ttf).unwrap().post().unwrap();
    assert_ne!(post.is_fixed_pitch(), 0);
}

/// The default PANOSE claimed `bProportion = 9` (Monospaced) unconditionally.
/// Windows GDI takes that claim literally and lays every glyph out on one cell
/// — the font preview and Notepad drew a 0.5em space at roughly 1.9em. It is a
/// claim about the design, so an undeclared PANOSE must not make it.
///
/// It is deliberately *not* tied to `meta fixed-pitch`. That flag is
/// `post.isFixedPitch` alone, which Windows tolerates on a font whose advances
/// differ (`font/Unison.unf` declares it so terminal font pickers list a font
/// with two cell widths); PANOSE 9 is what GDI acts on. A font that really
/// wants the PANOSE claim declares the whole `meta panose`.
#[test]
fn default_panose_never_claims_monospace() {
    let panose = |src: &str| {
        let ttf = build_from(src);
        read_fonts::FontRef::new(&ttf)
            .unwrap()
            .os2()
            .unwrap()
            .panose_10()[3]
    };
    assert_ne!(panose("glyph a 1 1\n@@\nmap A = a\n"), 9);
    assert_ne!(
        panose("meta fixed-pitch\nglyph a 1 1\n@@\nmap A = a\n"),
        9,
        "`fixed-pitch` is post"
    );

    // A declared PANOSE is the whole PANOSE, monospace claim included.
    let ttf = build_from(
        "meta fixed-pitch\nmeta panose 2 11 6 9 2 2 2 2 2 4\nglyph a 1 1\n@@\nmap A = a\n",
    );
    let os2 = read_fonts::FontRef::new(&ttf).unwrap().os2().unwrap();
    assert_eq!(os2.panose_10(), &[2, 11, 6, 9, 2, 2, 2, 2, 2, 4][..]);
    assert_ne!(
        read_fonts::FontRef::new(&ttf)
            .unwrap()
            .post()
            .unwrap()
            .is_fixed_pitch(),
        0,
        "and it does not disturb `fixed-pitch`"
    );
}

/// `head.flags` bit 4 tells a rasterizer the hinting may move the advance, so
/// it must not use the linear advance and — with no `LTSH`/`hdmx` to consult —
/// runs the hint program of every glyph to find one. Windows GDI is the engine
/// that honors this. The grid-snap hints in `hints.rs` only `SHPIX` contour
/// points and never touch a phantom point, so the claim was false and the work
/// it asks for is wasted.
#[test]
fn head_does_not_claim_the_hints_alter_the_advance() {
    let ttf = build_from("glyph a 2 2\n@@\n@@\nmap A = a\n");
    let flags = read_fonts::FontRef::new(&ttf)
        .unwrap()
        .head()
        .unwrap()
        .flags()
        .bits();
    assert_eq!(flags & 0x0010, 0, "INSTRUCTIONS_MAY_ALTER_ADVANCE_WIDTH");
    assert_eq!(flags, 0x0003, "baseline at y=0 and lsb at x=0 still hold");
}

/// A style flag has to reach both `OS/2.fsSelection` and `head.macStyle`, and
/// it has to clear the REGULAR bit — a font claiming bold *and* regular is a
/// classic and very visible bug.
#[test]
fn style_flags_reach_both_tables_and_clear_regular() {
    let ttf = build_from("meta bold\nmeta italic\nglyph a 1 1\n@@\nmap A = a\n");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    let sel = font.os2().unwrap().fs_selection().bits();
    assert_ne!(sel & 0x01, 0, "ITALIC");
    assert_ne!(sel & 0x20, 0, "BOLD");
    assert_eq!(sel & 0x40, 0, "REGULAR must be cleared");
    assert_eq!(
        font.head().unwrap().mac_style().bits() & 0b11,
        0b11,
        "bold|italic"
    );

    let ttf = build_from("glyph a 1 1\n@@\nmap A = a\n");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    assert_ne!(
        font.os2().unwrap().fs_selection().bits() & 0x40,
        0,
        "REGULAR by default"
    );
    assert_eq!(font.head().unwrap().mac_style().bits(), 0);
}

/// Values that live on the pixel grid are declared in pixels and scaled by the
/// same `UNITS_PER_EM / height` everything else uses.
#[test]
fn pixel_valued_metrics_are_scaled_to_font_units() {
    let ttf = build_from(
        "meta height 16\nmeta ascent 14\nmeta descent 2\n\
         meta cap-height 10\nmeta x-height 7\nmeta line-gap 2\n\
         meta underline-at -2 1\nmeta strikeout-at 1 5\n\
         glyph a 1 1\n@@\nmap A = a\n",
    );
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    let os2 = font.os2().unwrap();
    // UNITS_PER_EM is 1024 over a 16px em, so one pixel is 64 units.
    assert_eq!(os2.s_cap_height(), Some(640));
    assert_eq!(os2.sx_height(), Some(448));
    assert_eq!(os2.s_typo_line_gap(), 128);
    assert_eq!(font.hhea().unwrap().line_gap().to_i16(), 128);
    assert_eq!(os2.y_strikeout_size(), 64);
    assert_eq!(os2.y_strikeout_position(), 320);
    let post = font.post().unwrap();
    assert_eq!(post.underline_position().to_i16(), -128);
    assert_eq!(post.underline_thickness().to_i16(), 64);
}

#[test]
fn weight_width_and_panose_are_declared() {
    let ttf = build_from(
        "meta weight 700\nmeta width 3\nmeta panose 2 11 6 9 2 2 2 2 2 4\n\
         glyph a 1 1\n@@\nmap A = a\n",
    );
    let os2 = read_fonts::FontRef::new(&ttf).unwrap().os2().unwrap();
    assert_eq!(os2.us_weight_class(), 700);
    assert_eq!(os2.us_width_class(), 3);
    assert_eq!(os2.panose_10(), &[2, 11, 6, 9, 2, 2, 2, 2, 2, 4][..]);
}

/// The built cmap is the *face's* cmap: a mapping in a slice the face does not
/// include must not reach it. Getting this wrong would silently ship one
/// typeface with another's character mapping.
#[test]
fn a_slice_the_face_excludes_does_not_reach_the_cmap() {
    let src = "\
slice narrow
slice wide
glyph n 1 1
@@
glyph w 2 1
@@@@
map narrow : ° = n
map wide : ° = w
map A = n
";
    // Compared by advance rather than by glyph id: a glyph reachable from two
    // codepoints gets one entry per codepoint, so equal ids would not mean what
    // it looks like they mean. `n` is one pixel wide and `w` two, and one pixel
    // is 64 units at the default 16-pixel em.
    let advance_of = |ttf: &[u8], ch: char| -> u16 {
        let font = read_fonts::FontRef::new(ttf).unwrap();
        let gid = font
            .cmap()
            .unwrap()
            .map_codepoint(ch)
            .expect("must be mapped");
        font.hmtx().unwrap().advance(gid).unwrap()
    };

    let ttf = build_from(&format!("{src}face narrow : narrow\n"));
    assert_eq!(
        advance_of(&ttf, '°'),
        64,
        "the narrow face maps ° to the narrow glyph"
    );
    assert_eq!(advance_of(&ttf, 'A'), 64, "the base slice is in both faces");

    // The same source with the other face selected picks the other glyph.
    let ttf = build_from(&format!("{src}face wide : wide\n"));
    assert_eq!(
        advance_of(&ttf, '°'),
        128,
        "the wide face maps ° to the wide glyph"
    );
    assert_eq!(advance_of(&ttf, 'A'), 64);
}

/// A slice-qualified line may name several slices, and a slice-scoped
/// `name-parts` gives each of them its own spelling of the target — which is
/// how a naming scheme that differs by one suffix is written once instead of
/// once per slice. The slices are an outer loop: `⓪|①` below expands against
/// the codepoints in each slice separately, not zipped against them.
#[test]
fn a_slice_scoped_name_part_states_one_map_per_slice() {
    let src = "\
slice narrow
slice wide
name-parts wide : $half = ``
name-parts narrow : $half = -half
glyph w 2 1
@@@@
glyph w-half 1 1
@@
glyph z 2 1
@@@@
glyph z-half 1 1
@@
map wide|narrow : ⓪|① = (w|z)($half)
";
    let advance_of = |ttf: &[u8], ch: char| -> u16 {
        let font = read_fonts::FontRef::new(ttf).unwrap();
        let gid = font
            .cmap()
            .unwrap()
            .map_codepoint(ch)
            .expect("must be mapped");
        font.hmtx().unwrap().advance(gid).unwrap()
    };

    let ttf = build_from(&format!("{src}face wide : wide\n"));
    assert_eq!(advance_of(&ttf, '⓪'), 128, "the wide face gets `w`");
    assert_eq!(advance_of(&ttf, '①'), 128, "...and `z`");

    let ttf = build_from(&format!("{src}face term : narrow\n"));
    assert_eq!(advance_of(&ttf, '⓪'), 64, "the narrow face gets `w-half`");
    assert_eq!(advance_of(&ttf, '①'), 64, "...and `z-half`");
}

/// A character mapped in both the base slice and an included slice is an
/// `issues.rs` error, but the builder still has to survive it: this used to
/// `unwrap()` a `CmapConflict` deep in the table stage, and the panic killed
/// the background build thread — so the editor's build stalled for good and the
/// diagnostic that explains the mistake never reached the user.
#[test]
fn a_conflicting_cmap_entry_does_not_panic_the_build() {
    let ttf = build_from(
        "\
slice wide
glyph b 1 1
@@
glyph w 2 1
@@@@
map • = b
map wide : • = w
face regular : wide
",
    );
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    // The winner is the first-collected glyph, so the cmap stays deterministic
    // rather than depending on which document happened to be parsed first.
    let gid = font
        .cmap()
        .unwrap()
        .map_codepoint('•')
        .expect("must still be mapped");
    assert_eq!(font.hmtx().unwrap().advance(gid).unwrap(), 64);
}

/// One glyph is one glyph however many characters reach it. The collector used
/// to emit an entry per `(codepoint, glyph)` pair, so a glyph mapped twice was
/// stored twice — and, worse, the glyph order then depended on the cmap, which
/// is what stopped two faces from sharing `glyf`.
#[test]
fn a_glyph_mapped_from_two_codepoints_gets_one_gid() {
    let ttf = build_from("glyph sp 1 1\n..\nmap U+0020 = sp\nmap U+00A0 = sp\nmap A = sp\n");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    let cmap = font.cmap().unwrap();
    let a = cmap.map_codepoint(' ').unwrap();
    let b = cmap.map_codepoint('\u{00A0}').unwrap();
    let c = cmap.map_codepoint('A').unwrap();
    assert_eq!(a, b);
    assert_eq!(a, c);
    // .notdef plus the one glyph.
    assert_eq!(font.maxp().unwrap().num_glyphs(), 2);
}

/// TrueType reserves GID 0 for `.notdef`, the glyph a renderer draws for a
/// character the font does not cover, so a `.notdef` the source draws has to
/// land *there*. It used to be collected like any other name — which meant an
/// unmapped, unreferenced `.notdef` was dropped as unused while GID 0 stayed
/// the builder's hardcoded blank, and the drawn tofu never reached the font.
#[test]
fn source_notdef_becomes_gid_zero() {
    let ttf = build_from("glyph .notdef 2 2\n@@@@\n@@@@\nglyph a 2 2\n@@..\n..@@\nmap A = a\n");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    // `.notdef` occupies the reserved slot rather than adding one of its own.
    assert_eq!(font.maxp().unwrap().num_glyphs(), 2);
    let glyf = font.glyf().unwrap();
    let loca = font.loca(None).unwrap();
    assert!(
        loca.get_glyf(GlyphId::new(0), &glyf).unwrap().is_some(),
        "GID 0 must carry the outline the source drew for .notdef",
    );
    // ... and the mapped glyph still follows it rather than being displaced.
    assert_eq!(font.cmap().unwrap().map_codepoint('A').unwrap().to_u32(), 1);
    assert!(loca.get_glyf(GlyphId::new(1), &glyf).unwrap().is_some());
}

/// The other half of the same rule: with no `.notdef` in the source, GID 0 is
/// still reserved and still blank, and no glyph is shifted off it.
#[test]
fn absent_notdef_leaves_gid_zero_blank() {
    let ttf = build_from("glyph a 2 2\n@@..\n..@@\nmap A = a\n");
    let font = read_fonts::FontRef::new(&ttf).unwrap();
    assert_eq!(font.maxp().unwrap().num_glyphs(), 2);
    let glyf = font.glyf().unwrap();
    let loca = font.loca(None).unwrap();
    assert!(
        loca.get_glyf(GlyphId::new(0), &glyf).unwrap().is_none(),
        "GID 0 stays blank when the source draws no .notdef",
    );
    assert_eq!(font.cmap().unwrap().map_codepoint('A').unwrap().to_u32(), 1);
}

/// `.notdef` is kept without being asked for: nothing maps or refs it, so the
/// ordinary retention rules would drop it before it ever reached GID 0.
#[test]
fn notdef_is_kept_although_nothing_names_it() {
    let doc = document_io::parse_document_from_str(
        "glyph .notdef 2 2\n@@@@\n@@@@\nglyph a 2 2\n@@..\n..@@\nmap A = a\n",
        "test.unf".into(),
    )
    .unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    assert_eq!(
        glyphs.first().map(|g| g.name.as_str()),
        Some(".notdef"),
        "`.notdef` is collected, and first: {:?}",
        glyphs.iter().map(|g| &g.name).collect::<Vec<_>>(),
    );
    assert!(!glyphs[0].contours.is_empty(), "with its own outline");
}

#[test]
fn unmapped_empty_sticky_glyph_is_retained() {
    let doc =
        document_io::parse_document_from_str("glyph keep sticky advance 0\n", "test.unf".into())
            .unwrap();
    let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
    let keep = glyphs.iter().find(|glyph| glyph.name == "keep").unwrap();
    assert!(keep.codepoints.is_empty());
    assert_eq!(keep.advance_width, 0);
    assert!(keep.contours.is_empty());
}

#[test]
fn meta_height_zero_returns_none() {
    let doc = document_io::parse_document_from_str(
        "meta height 0\nmeta ascent 0\nmeta descent 0\nglyph a 1 1\n@@\nmap A = a\n",
        "test.unf".into(),
    )
    .unwrap();
    let result = build_font_from_documents(&[&doc]);
    assert!(result.is_none(), "height 0 should reject build");
}

#[test]
fn parse_map_char_accepts_lowercase_u_plus() {
    assert_eq!(parse_map_char("u+0041"), Some(0x41));
    assert_eq!(parse_map_char("U+0041"), Some(0x41));
}

#[test]
fn expand_map_pairs_depth_aware_pipe_split() {
    // g(a|b) has a pipe inside parens — must not be split at top level
    let pairs = expand_map_pairs("A|B", "g(a|b)");
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0], (0x41, "ga".to_string()));
    assert_eq!(pairs[1], (0x42, "gb".to_string()));
}

#[test]
fn expand_map_pairs_cycles_glyph_names() {
    // 3 chars, 2 glyph names — should cycle
    let pairs = expand_map_pairs("A|B|C", "ga|gb");
    assert_eq!(pairs.len(), 3);
    assert_eq!(pairs[2].1, "ga");
}

#[test]
fn expand_map_pairs_single_char_expands_pattern() {
    let pairs = expand_map_pairs("A", "g(a|b)");
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0], (0x41, "ga".to_string()));
}

#[test]
fn expand_map_pairs_lowercase_u_plus_list() {
    let pairs = expand_map_pairs("u+0041|u+0042", "ga|gb");
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, 0x41);
    assert_eq!(pairs[1].0, 0x42);
}

#[test]
fn expand_map_pairs_reverse_range_returns_empty() {
    let pairs = expand_map_pairs("U+0042..0041", "g(a|b)");
    assert!(pairs.is_empty());
}

#[test]
fn expand_map_pairs_bare_pipe_char() {
    // "map | = pipe" — the pipe character itself, not a separator
    let pairs = expand_map_pairs("|", "pipe");
    assert_eq!(pairs, vec![('|' as u32, "pipe".to_string())]);
}

#[test]
fn mark_flag_roundtrips() {
    let input = "\
glyph dia 3 2 mark
@@@@@@
@@@@@@
anchor -above 1 1
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc.items[0] {
        assert!(body.mark, "mark flag should be parsed");
    } else {
        panic!("expected glyph");
    }

    let mut output = Vec::new();
    document_io::serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(
        output_str.contains("mark"),
        "mark flag should be serialized"
    );

    let doc2 = document_io::parse_document_from_str(&output_str, "test.unf".into()).unwrap();
    if let DocumentItem::Glyph { body, .. } = &doc2.items[0] {
        assert!(body.mark, "mark flag should survive roundtrip");
    }
}

#[test]
fn feature_anchor_roundtrips() {
    let input = "\
feature ccmp for DFLT latn : anchor above
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    if let DocumentItem::FeatureAnchor {
        name,
        scripts,
        anchor,
        ..
    } = &doc.items[0]
    {
        assert_eq!(name, "ccmp");
        assert_eq!(scripts, &["DFLT", "latn"]);
        assert_eq!(anchor, "above");
    } else {
        panic!("expected FeatureAnchor, got {:?}", doc.items[0]);
    }

    let mut output = Vec::new();
    document_io::serialize_document(&doc, &mut output).unwrap();
    let output_str = String::from_utf8(output).unwrap();
    assert!(output_str.contains("anchor above"));
}

/// Regression test: `glyph_bounds` on an empty contour list must return
/// a degenerate (0,0,0,0) box, not (MAX,MAX,MIN,MIN) — the latter is an
/// invalid bbox (x_min > x_max) that Chrome's OTS sanitizer rejects.
#[test]
fn glyph_bounds_empty_contours_is_degenerate_zero_box() {
    assert_eq!(glyph_bounds(&[]), (0, 0, 0, 0));
}

/// Firefox reports every `maxp` limit the outlines actually exceed
/// ("Component depth exceeds maxp maxComponentDepth", "Number of composite
/// points … exceeds maxp maxCompositePoints", "Number of contour points
/// exceeds maxp maxPoints").  Composite totals must be counted after full
/// decomposition, nesting depth must be measured rather than assumed to be
/// 1, and COLR layer glyphs must be counted at all.
#[test]
fn maxp_limits_cover_the_emitted_outlines() {
    // A merged foreground COLR layer made of on-demand pieces: they are
    // inlined, so the layer's points exist nowhere else in the font.
    let color_refs: String = (0..10)
        .flat_map(|i| [(i * 2, 0), (i * 2, 2)])
        .map(|(c, r)| format!("ref 1x1 {c} {r} coloronly\n"))
        .collect();
    let input = format!(
        "\
meta height 16
meta ascent 12
meta descent 4

// diagonal edges gain intermediate points when the glyph is emitted with
// grid-snap hints, so a component carries more points than the parent's own
// pre-hinting outline suggests
glyph tri 4 4
/1@@@@@@
../1@@@@
..../1@@
....../1

// three levels of nesting.  Each level is mapped on purpose: a glyph pulled
// into the font only as a component is collected without its own `ref`s and
// would flatten into a simple outline instead of nesting.
glyph nest1
ref tri
ref tri 4 0

glyph nest2
ref nest1
ref nest1 8 0

glyph nest3
ref nest2
ref nest2 16 0

color red = #ff0000

glyph colored 20 4
@@......................................
........................................
........................................
........................................
{color_refs}ref 1x1 0 3 coloronly fill red

map A = nest1
map B = nest2
map C = nest3
map D = colored
"
    );
    let doc = document_io::parse_document_from_str(&input, "t.unf".into()).unwrap();
    let bytes = build_font_from_documents(&[&doc]).expect("font should build");

    let want = recomputed_maxp(&bytes);
    let font = read_fonts::FontRef::new(&bytes).unwrap();
    let maxp = font.maxp().unwrap();
    let got: HashMap<&'static str, u16> = HashMap::from([
        ("maxPoints", maxp.max_points().unwrap()),
        ("maxContours", maxp.max_contours().unwrap()),
        ("maxCompositePoints", maxp.max_composite_points().unwrap()),
        (
            "maxCompositeContours",
            maxp.max_composite_contours().unwrap(),
        ),
        (
            "maxComponentElements",
            maxp.max_component_elements().unwrap(),
        ),
        ("maxComponentDepth", maxp.max_component_depth().unwrap()),
    ]);

    // The fixture has to actually exercise each limit, or the comparison
    // below would pass on a font that never nests or never colors.
    assert_eq!(
        want["maxComponentDepth"], 3,
        "fixture should nest three deep"
    );

    for key in got.keys() {
        assert_eq!(
            got[key], want[key],
            "maxp {key}: stored {} but the outlines need {}",
            got[key], want[key],
        );
    }
}

/// The DOT+two-corner shapes and their complements, traced alone in a single
/// cell. The shape itself is one connected ring; the complement is the two
/// corners left over, which are either disjoint (two rings) or meet at an
/// edge midpoint, where the tracer walks through the pinch point as it does
/// for the inverse cones. Either way the diamond stays a hole.
#[test]
fn dot_corner_shapes_and_their_complements_trace_as_expected() {
    for (code, rings, points, covers_center) in [
        ("d/", 1, 6, true),  // SLASH
        ("\\b", 1, 6, true), // BACKSLASH
        ("1D", 1, 5, true),  // HOUSE1
        ("1v", 1, 5, true),  // HOUSE2
        ("C1", 1, 5, true),  // HOUSE3
        ("^1", 1, 5, true),  // HOUSE4
        // Two disjoint triangles: two rings.
        ("~_", 2, 3, false), // INVSLASH
        ("_~", 2, 3, false), // INVBACKSLASH
        // Two triangles meeting at an edge midpoint: the tracer walks
        // through the pinch point, as it does for the inverse cones.
        (".)", 1, 5, false), // INVHOUSE1
        ("M1", 1, 5, false), // INVHOUSE2
        ("(.", 1, 5, false), // INVHOUSE3
        ("1W", 1, 5, false), // INVHOUSE4
    ] {
        let src = format!(
            "meta height 16\nmeta ascent 12\nmeta descent 4\n\nglyph t 1 1\n{code}\n\nmap A = t\n"
        );
        let doc = document_io::parse_document_from_str(&src, "test.unf".into()).unwrap();
        let (_, _, glyphs, _, _) = collect_glyph_data(&[&doc], false).unwrap();
        let glyph = glyphs.iter().find(|g| g.name == "t").unwrap();
        assert_eq!(glyph.contours.len(), rings, "{code}: {:?}", glyph.contours);
        for c in &glyph.contours {
            assert_eq!(simplify_collinear(c).len(), points, "{code}: {c:?}");
        }
        // The cell is 64 units wide and its centre is the middle of the
        // diamond: filled for the shape, a hole for the complement.
        let centre = winding_at(&glyph.contours, 32.0, 736.0) != 0;
        assert_eq!(centre, covers_center, "{code}: {:?}", glyph.contours);
    }
}

/// A cancelled font build produces nothing at all — not a partial font, and
/// not one the editor could mistake for current.
///
/// The editor cancels a build the moment its document set is superseded, so
/// this is the ordinary outcome of clicking pixels quickly; without it, each
/// click's build ran to the end and the next one queued behind it on the
/// contour cache.
#[test]
fn a_cancelled_build_produces_no_font() {
    let input = "\
meta height 4
meta ascent 3
meta descent 1

glyph a 2 2
@@
.@

map A = a
";
    let doc = document_io::parse_document_from_str(input, "test.unf".into()).unwrap();
    let cache = crate::render::new_contour_cache();

    let cancel = crate::cancel::CancelToken::new();
    cancel.cancel();
    assert!(
        build_font_pair_cached_for(&[&doc], &cache, None, &cancel).is_none(),
        "a cancelled build must not hand back a font"
    );

    // And the shared cache survives it: an aborted build skips `evict_stale`,
    // which is a cache holding more than it needs, never a broken one. The
    // build that replaces it has to succeed against that same cache.
    let built =
        build_font_pair_cached_for(&[&doc], &cache, None, &crate::cancel::CancelToken::never())
            .expect("the replacing build still succeeds on the cache the cancelled one left");
    assert!(!built.bitmap.is_empty() && !built.vector.is_empty());
}
