//! Tests for [`crate::specimen`].
//!
//! Declared as a child module of that one through `#[path]`, so it still
//! reaches its private items.

use super::*;

fn doc(src: &str) -> Document {
    crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap()
}

const SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
name-parts $l = a b
glyph sq 1 1
@@
glyph a-lig
ref sq
glyph b-lig
ref sq
map U+0061 = sq
remap liga : sq -> ($l)-lig
";

/// The gid map and `name_parts` arrive from *background* work, so the
/// specimen can be opened while both are still the previous build's (or,
/// at startup, empty). Keying its cache on the build request would then
/// freeze that half-built state in place forever.
#[test]
fn rebuilds_when_name_parts_and_gids_arrive_late() {
    let d = doc(SRC);
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let gids: HashMap<String, u16> = [
        ("sq".to_string(), 1u16),
        ("a-lig".to_string(), 2),
        ("b-lig".to_string(), 3),
    ]
    .into_iter()
    .collect();

    let mut state = SpecimenState::new();

    // Frame 1: opened before the background build landed — no name parts,
    // no gid map yet.
    assert!(state.needs_rebuild(0, 0));
    state.rebuild_if_needed(
        &docs,
        &NamePartsMap::new(),
        &HashMap::new(),
        None,
        &GlyphFlags::default(),
        0,
        0,
    );
    assert!(state.remap_glyph_names().is_empty());

    // Frame 2: the build and the derived data have landed.
    assert!(state.needs_rebuild(1, 1));
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &gids,
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    assert_eq!(state.remap_glyph_names(), vec!["a-lig", "b-lig"]);

    // Nothing new: the cache holds.
    assert!(!state.needs_rebuild(1, 1));
}

/// The `prop` lines reach the hover status through the same rebuild as
/// everything else — one generation behind an edit, never stale after it.
#[test]
fn a_rebuild_picks_up_the_prop_lines() {
    let d = doc(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph logo 1 1\n",
        "@@\n",
        "map U+E000 = logo\n",
        "prop U+E000 = `UNISON LOGO` gc So eaw W\n",
    ));
    let docs = [&d];
    let mut state = SpecimenState::new();
    assert_eq!(state.char_props.name(0xE000), None);

    state.rebuild_if_needed(
        &docs,
        &NamePartsMap::new(),
        &HashMap::new(),
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    assert_eq!(
        state.char_props.name(0xE000).as_deref(),
        Some("UNISON LOGO")
    );
    assert_eq!(
        state.char_props.property_summary('\u{E000}'),
        "{gc=So eaw=W}"
    );
}

const SLICED_SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
face regular : wide
face term : narrow
slice narrow
slice wide
name-parts wide : $-half = ``
name-parts narrow : $-half = -half
glyph star 1 1
@@
glyph star-half 1 1
@@
map wide|narrow : U+2042 = star($-half)
";

/// Builds a state over `src` as if the background pipeline had landed,
/// which is all the grid layout needs — it reads no font bytes.
fn state(src: &str) -> SpecimenState {
    let d = doc(src);
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &HashMap::new(),
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    state
}

/// A tinted cell sends a click to the line the fault is on, not to the
/// glyph the character maps to — for a Han character the latter is a
/// pattern line covering a whole block.
#[test]
fn a_click_on_a_faulted_cell_goes_to_the_glyph_the_fault_is_in() {
    let d = doc("\
meta height 2
meta ascent 2
meta descent 0
glyph part 1 1
@
glyph whole 1 1
ref part
ref nowhere
map U+0041 = whole
");
    let docs = [&d];
    let resolution = crate::resolve::Resolution::compute(&docs);
    let issues = crate::issues::collect_issues_with(&docs, &resolution);
    let flags = crate::glyph_flags::collect(&docs, &issues, &resolution.expansion);
    let name_parts = crate::document::collect_name_parts(&docs);
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(&docs, &name_parts, &HashMap::new(), None, &flags, 1, 1);
    state.rebuild_sections();

    let cell = *state
        .items
        .iter()
        .find(|i| matches!(i, Item::Char(n) if state.entries[*n].cp == 0x41))
        .expect("U+0041 is on the grid");
    assert_eq!(state.flag_for(cell), Some(GlyphFlag::Error));
    // `whole` is faulted directly, so nothing is redirected.
    assert_eq!(state.goto_target(cell), Some("whole"));
    assert!(state.status_for(cell).ends_with(" \u{2014} error"));
}

/// …and when the fault is one level down, the click follows it there while
/// the tint stays on the cell the character reaches.
#[test]
fn an_inherited_fault_redirects_the_click_and_says_so() {
    let d = doc("\
meta height 2
meta ascent 2
meta descent 0
glyph part 1 1
ref nowhere
glyph whole 1 1
ref part
map U+0041 = whole
");
    let docs = [&d];
    let resolution = crate::resolve::Resolution::compute(&docs);
    let issues = crate::issues::collect_issues_with(&docs, &resolution);
    let flags = crate::glyph_flags::collect(&docs, &issues, &resolution.expansion);
    let name_parts = crate::document::collect_name_parts(&docs);
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(&docs, &name_parts, &HashMap::new(), None, &flags, 1, 1);
    state.rebuild_sections();

    let cell = *state
        .items
        .iter()
        .find(|i| matches!(i, Item::Char(n) if state.entries[*n].cp == 0x41))
        .expect("U+0041 is on the grid");
    assert_eq!(state.flag_for(cell), Some(GlyphFlag::Error));
    assert_eq!(state.goto_target(cell), Some("part"));
    assert!(
        state
            .status_for(cell)
            .ends_with(" \u{2014} error in 'part'")
    );
}

const BLOCKS_SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
glyph sq 1 1
@@
map U+0041 = sq
map U+0042 = sq
map U+2200 = sq
";

/// Off, the grid is one unheaded run of every mapped character; on, it is
/// one section per block, in code point order.
#[test]
fn grouping_gives_each_block_a_heading_row() {
    let mut state = state(BLOCKS_SRC);
    state.options.group_by_block = false;
    assert_eq!(state.row_summaries(2), vec!["0041 0042", "2200"]);

    state.options.group_by_block = true;
    assert_eq!(
        state.row_summaries(2),
        vec![
            "# Basic Latin  U+0000..007F  2 / 95 (2.1%)",
            "0041 0042",
            "# Mathematical Operators  U+2200..22FF  1 / 256 (0.4%)",
            "2200",
        ]
    );
}

/// The grid opens grouped by block and without the metric marks: the
/// headings are what makes a few thousand cells readable, and the marks are
/// a detail to turn on for one look, not the resting state.
#[test]
fn the_grid_opens_grouped_and_unmarked() {
    let options = SpecimenOptions::default();
    assert!(options.group_by_block);
    assert!(!options.show_metric_marks);
    assert!(!options.show_undeclared);
}

/// A heading's coverage counts the block's *characters*, not its cells:
/// the count is the same whether or not the grid is filled, and a block's
/// permanent holes are in neither side of the fraction.
#[test]
fn a_heading_states_its_block_coverage() {
    // Greek and Coptic (U+0370..03FF) is 144 code points with 9 permanent
    // holes, so 135 characters.
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "map U+0370..0372 = sq\n",
    ));
    let heading = |s: &mut SpecimenState| s.row_summaries(8)[0].clone();
    assert_eq!(
        heading(&mut state),
        "# Greek and Coptic  U+0370..03FF  3 / 135 (2.2%)"
    );
    state.options.show_undeclared = true;
    assert_eq!(
        heading(&mut state),
        "# Greek and Coptic  U+0370..03FF  3 / 135 (2.2%)"
    );
}

/// A control (`gc=Cc`) is not a character the font can be missing: it is in
/// neither side of a heading's fraction, and a filled grid gives it no cell.
#[test]
fn controls_are_neither_counted_nor_listed() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "map U+0041 = sq\n",
    ));
    // Basic Latin is 128 code points, 33 of which are controls, so 95
    // characters.
    let heading = |s: &mut SpecimenState| s.row_summaries(8)[0].clone();
    assert_eq!(
        heading(&mut state),
        "# Basic Latin  U+0000..007F  1 / 95 (1.1%)"
    );
    state.options.show_undeclared = true;
    let rows = state.row_summaries(8);
    let cells: Vec<&String> = rows.iter().filter(|r| !r.starts_with('#')).collect();
    assert!(
        !cells
            .iter()
            .any(|r| r.contains("0000") || r.contains("007F")),
        "a control got a cell: {cells:?}"
    );
}

/// Remap-only glyphs have no code point to sort among the blocks, so they
/// come last — under a heading of their own once the grid is grouped.
#[test]
fn remap_only_glyphs_are_a_section_of_their_own() {
    let mut state = SpecimenState::new();
    let d = doc(SRC);
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let gids: HashMap<String, u16> = [("a-lig".to_string(), 2u16), ("b-lig".to_string(), 3)]
        .into_iter()
        .collect();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &gids,
        None,
        &GlyphFlags::default(),
        1,
        1,
    );

    state.options.group_by_block = false;
    assert_eq!(state.row_summaries(4), vec!["0061 a-lig b-lig"]);
    state.options.group_by_block = true;
    assert_eq!(
        state.row_summaries(4),
        vec![
            "# Basic Latin  U+0000..007F  1 / 95 (1.1%)",
            "0061",
            // The remaps have no block, so there is nothing to be a
            // fraction of.
            "# Remaps",
            "a-lig b-lig",
        ]
    );
}

/// A `prop block` claim is what names an area of the Private Use planes;
/// the UCD calls all of it "Supplementary Private Use Area-A".
#[test]
fn a_stated_block_names_its_own_section() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph logo 1 1\n",
        "@@\n",
        "prop block `Unison Symbols` = U+F0000..F000F\n",
        "map U+F0000 = logo\n",
    ));
    state.options.group_by_block = true;
    // U+F0000 is drawn but no `prop` line states it, so it is a character
    // the block does not have — see
    // `a_block_whose_glyphs_outrun_its_characters_states_no_coverage`.
    assert_eq!(
        state.row_summaries(4),
        vec!["# Unison Symbols  U+F0000..F000F", "F0000"]
    );
}

/// Inside Private Use the `prop` lines are the UCD: the UCD's own `Co` says
/// only that the code point is *available*, so a `prop block` claim names an
/// area and the `prop` lines in it say which of its characters exist. The
/// coverage counts those, not the claim's whole range.
#[test]
fn a_private_use_block_counts_the_characters_its_prop_lines_state() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph logo 1 1\n",
        "@@\n",
        "prop block `Unison Symbols` = U+F0000..F000F\n",
        "prop U+F0000 = `UNISON ONE`\n",
        "prop U+F0001 = `UNISON TWO`\n",
        "prop U+F0002 = `UNISON THREE`\n",
        "map U+F0000 = logo\n",
    ));
    state.options.group_by_block = true;
    assert_eq!(
        state.row_summaries(4)[0],
        "# Unison Symbols  U+F0000..F000F  1 / 3 (33.3%)"
    );
    // Filling shows the stated characters, not the claimed range.
    state.options.show_undeclared = true;
    assert_eq!(state.cell_cps(), vec![0xF0000, 0xF0001, 0xF0002]);
}

/// A character the source draws but states no `prop` for is not one of the
/// block's characters, so counting it would put the coverage over 100%.
/// Rather than a fraction that cannot be read, such a block states none —
/// which is the same rule outside Private Use, where a `map` onto one of a
/// block's permanent holes is the way to get there.
#[test]
fn a_block_whose_glyphs_outrun_its_characters_states_no_coverage() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph logo 1 1\n",
        "@@\n",
        "prop block `Unison Symbols` = U+F0000..F000F\n",
        "prop U+F0000 = `UNISON ONE`\n",
        // Drawn, but nothing says this character exists.
        "map U+F0001 = logo\n",
        // U+0378 is a permanent hole of Greek and Coptic.
        "map U+0370 = logo\n",
        "map U+0378 = logo\n",
    ));
    state.options.group_by_block = true;
    let headings: Vec<String> = state
        .row_summaries(4)
        .into_iter()
        .filter(|r| r.starts_with('#'))
        .collect();
    assert_eq!(
        headings,
        vec![
            "# Greek and Coptic  U+0370..03FF",
            "# Unison Symbols  U+F0000..F000F",
        ]
    );
}

/// Filling a block puts a cell on every character it *has*, and the UCD
/// says which those are — except in Private Use, where it assigns all
/// 137,468 code points and describes none. There, only the source speaks:
/// a `map` or a `prop` line is what makes a Private Use character exist,
/// and a `prop block` claim names an area without populating it. Filling
/// from the claim instead used to put 256 mostly-empty cells on the grid
/// per claimed block, which is exactly the unassigned bulk that filling
/// leaves out everywhere else.
#[test]
fn filling_never_invents_a_private_use_character() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph logo 1 1\n",
        "@@\n",
        // A claim of four code points, one drawn…
        "prop block `Unison Symbols` = U+F0000..F0003\n",
        "map U+F0000 = logo\n",
        // …and two more characters out in unclaimed Private Use: one mapped,
        // one only named.
        "map U+F1000 = logo\n",
        "prop U+F1001 = `UNISON SPARE`\n",
    ));
    state.options.show_undeclared = true;
    assert_eq!(state.cell_cps(), vec![0xF0000, 0xF1000, 0xF1001]);
}

/// "Show undeclared characters" fills every block that has a mapped
/// character out to its whole range — but only with code points the UCD
/// assigns, since a block's permanent holes are not holes in the font.
#[test]
fn filling_a_block_skips_the_code_points_nothing_assigns() {
    // U+0370 GREEK CAPITAL LETTER HETA is in Greek and Coptic
    // (U+0370..03FF), whose U+0378 and U+0379 are permanent holes.
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "map U+0370 = sq\n",
    ));
    assert_eq!(state.cell_cps(), vec![0x370]);

    state.options.show_undeclared = true;
    let cps = state.cell_cps();
    assert_eq!(cps.first(), Some(&0x370));
    assert_eq!(cps.last(), Some(&0x3FF));
    assert!(cps.contains(&0x377));
    assert!(!cps.contains(&0x378));
    assert!(!cps.contains(&0x379));
    assert!(cps.contains(&0x37A));
    // The rest of the code space is untouched: no other block has a mapped
    // character to fill from.
    assert!(cps.iter().all(|cp| (0x370..=0x3FF).contains(cp)));
}

/// The fold, both halves of it: a filled grid folds a section longer than
/// twice [`FOLD_EDGE_ROWS`], and a grid showing only what the source drew
/// folds nothing — every row there is a glyph somebody wanted to look at.
#[test]
fn a_long_section_folds_only_while_filling() {
    let src = concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "map U+0041 = sq\n",
    );
    let mut state = state(src);
    state.options.group_by_block = false;
    state.options.show_undeclared = false;
    assert_eq!(state.row_summaries(2), vec!["0041"]);

    state.options.show_undeclared = true;
    let rows = state.row_summaries(2);
    // Basic Latin filled out is 48 rows at two columns: eight stay at each
    // end and the fold stands for the other 32.
    assert_eq!(rows.len(), 2 * FOLD_EDGE_ROWS + 1);
    assert_eq!(rows[FOLD_EDGE_ROWS], "\u{2026} 32");
    assert_eq!(rows[FOLD_EDGE_ROWS - 1], "002E 002F");
    assert_eq!(rows[FOLD_EDGE_ROWS + 1], "0070 0071");

    // Wide enough that the section is 16 rows or fewer, and nothing folds.
    let rows = state.row_summaries(8);
    assert!(!rows.iter().any(|r| r.starts_with('\u{2026}')));
}

/// A fold is opened by a click and stays open: the layout is what folded the
/// section, so the reader's answer lives beside it rather than in the source.
#[test]
fn an_opened_fold_lays_the_whole_section_out() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "map U+0041 = sq\n",
    ));
    state.options.group_by_block = true;
    state.options.show_undeclared = true;
    let folded = state.row_summaries(2);
    state.unfolded.insert(0);
    let opened = state.row_summaries(2);
    assert!(!opened.iter().any(|r| r.starts_with('\u{2026}')));
    // The heading is the one row the fold never took, so the difference is
    // exactly what the fold stood for.
    assert_eq!(opened.len(), folded.len() + 32 - 1);
    assert!(opened.contains(&"0042 0043".to_string()));
}

/// A folded section keeps its heading: a block worth folding is a block worth
/// seeing the name of.
#[test]
fn a_folded_block_keeps_its_heading() {
    let mut state = state(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "map U+0041 = sq\n",
        "map U+2200 = sq\n",
    ));
    state.options.group_by_block = true;
    state.options.show_undeclared = true;
    let rows = state.row_summaries(8);
    let heading = rows
        .iter()
        .position(|r| r.starts_with("# Mathematical Operators"))
        .expect("the block is on the grid");
    assert_eq!(
        rows[heading],
        "# Mathematical Operators  U+2200..22FF  1 / 256 (0.4%)"
    );
    assert!(
        rows[heading + 1 + FOLD_EDGE_ROWS].starts_with('\u{2026}'),
        "its 32 rows fold after the eighth"
    );
}

/// Only the mapped characters are clickable, so the fill has to record
/// which cells are which.
#[test]
fn a_filled_cell_knows_it_has_no_glyph() {
    let mut state = state(BLOCKS_SRC);
    state.options.show_undeclared = true;
    state.rebuild_sections();
    let by_cp: HashMap<u32, bool> = state
        .entries
        .iter()
        .map(|e| (e.cp, e.glyph_name.is_some()))
        .collect();
    assert!(by_cp[&0x41]);
    assert!(!by_cp[&0x40]);
}

/// A cell is dimmed when the built font draws nothing for it — an empty
/// grid, a `map` to a glyph that does not exist, or a character the source
/// declares nothing about. A blank glyph with an advance is *not* one of
/// those: a space is a character the font has.
#[test]
fn a_cell_with_no_metrics_is_the_one_the_font_draws_nothing_for() {
    let d = doc(concat!(
        "meta height 16\n",
        "meta ascent 14\n",
        "meta descent 2\n",
        "glyph sq 1 1\n",
        "@@\n",
        "glyph sp 8 16\n",
        "glyph nothing 0 0\n",
        "map U+0041 = sq\n",
        "map U+0020 = sp\n",
        "map U+0042 = nothing\n",
        "map U+0043 = absent\n",
    ));
    let bytes = crate::render::ttf_builder::build_font_from_documents(&[&d]).unwrap();
    let font = FontRef::new(&bytes).unwrap();
    assert!(cp_has_metrics(&font, 0x41));
    // A space is blank, but it is a character with a width.
    assert!(cp_has_metrics(&font, 0x20));
    // An empty grid, a `map` to nothing, and an undeclared character.
    assert!(!cp_has_metrics(&font, 0x42));
    assert!(!cp_has_metrics(&font, 0x43));
    assert!(!cp_has_metrics(&font, 0x44));
}

/// A slice-qualified `map` substitutes with *that slice's* name parts, so
/// the specimen has to expand it per slice like the builder does — and pick
/// the slice the face it is drawing actually includes. Expanding it with the
/// unqualified parts left `$-half` verbatim in the glyph name, which then
/// matched no glyph and made the cell unclickable.
#[test]
fn slice_scoped_name_parts_expand_per_face() {
    let d = doc(SLICED_SRC);
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let gids: HashMap<String, u16> = [("star".to_string(), 1u16), ("star-half".to_string(), 2)]
        .into_iter()
        .collect();

    let mut state = SpecimenState::new();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &gids,
        Some("regular"),
        &GlyphFlags::default(),
        1,
        1,
    );
    assert_eq!(state.glyph_for_cp(0x2042), Some("star"));

    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &gids,
        Some("term"),
        &GlyphFlags::default(),
        2,
        1,
    );
    assert_eq!(state.glyph_for_cp(0x2042), Some("star-half"));
}

const UVS_SRC: &str = "\
meta height 16
meta ascent 14
meta descent 2
glyph sq 1 1
@@
glyph var-a 1 1
@@
glyph var-b 1 1
@@
map U+4E00 = sq
map U+4E00 U+E0101 = var-b
map U+4E00 U+E0100 = var-a
map U+4E01 U+FE00 = var-a
";

/// A variation sequence is read against the character it varies, so its cell
/// follows that character's — in selector order, whatever order the source
/// states them in — and is labelled by its selector alone, the base being the
/// cell it shares an open box with.
#[test]
fn a_variation_sequence_follows_its_base_in_selector_order() {
    let mut state = state(UVS_SRC);
    state.options.group_by_block = false;
    assert_eq!(
        state.row_summaries(8),
        // U+4E01 is `map`ped by nothing on its own, and still gets the base
        // cell its sequence varies from.
        vec!["4E00 +VS17 +VS18 4E01 +VS1"]
    );
}

/// The undrawn borders: the ones a variation-sequence cell shares with the run
/// it belongs to. Read per row as one flag per vertical border, `cols + 1` of
/// them, so a run cut by a line break is left open at both ends of the cut.
#[test]
fn a_variation_sequence_is_joined_to_its_base_by_an_open_border() {
    let mut state = state(UVS_SRC);
    state.options.group_by_block = false;
    state.rebuild_sections();
    let dashes = |state: &SpecimenState, cols: usize| -> Vec<Vec<bool>> {
        let layout = state.build_layout(cols);
        layout
            .rows
            .iter()
            .filter_map(|row| match row {
                Row::Cells { start, len } => {
                    Some((0..=*len).map(|c| state.uvs_boundary(start + c)).collect())
                }
                _ => None,
            })
            .collect()
    };
    // `4E00  +VS17  +VS18 | 4E01  +VS1` — the two sequences of U+4E00 share
    // an open box with their base and each other, the next base does not, and
    // the outer edges of the row are drawn.
    assert_eq!(
        dashes(&state, 5),
        vec![vec![false, true, true, false, true, false]]
    );
    // Wrapped between the base and its first sequence: the right edge of the
    // first row and the left edge of the second are both left open.
    assert_eq!(
        dashes(&state, 1),
        vec![
            vec![false, true],
            vec![true, true],
            vec![true, false],
            vec![false, true],
            vec![true, false],
        ]
    );
}

/// The hover line names both halves of the sequence and the glyph it picks.
#[test]
fn a_variation_sequence_cell_states_the_pair_it_stands_for() {
    let mut state = state(UVS_SRC);
    state.options.group_by_block = false;
    state.rebuild_sections();
    let item = state.items[1];
    assert_eq!(
        state.status_body(item),
        "U+4E00 U+E0100 \u{4E00}\u{E0100} CJK UNIFIED IDEOGRAPH-4E00 + VS17 (var-a)"
    );
    // Ctrl+C over it copies the whole sequence: one half of it is not the
    // character anyone wanted.
    assert_eq!(state.copy_text(item).as_deref(), Some("\u{4E00}\u{E0100}"));
}

/// A glyph a variation sequence names is mapped, so it is not listed again as
/// a remap-only glyph — and an alias-named target is canonicalized like every
/// other `map` target, since the built font knows the glyph by one name only.
#[test]
fn a_variation_sequence_target_is_a_mapped_glyph() {
    let mut state = state(
        "\
meta height 16
meta ascent 14
meta descent 2
glyph sq 1 1
@@
glyph var-a 1 1
@@
glyph alias-a = var-a
map U+4E00 = sq
map U+4E00 U+E0100 = alias-a
remap liga : sq -> var-a
",
    );
    state.options.group_by_block = false;
    assert_eq!(state.row_summaries(8), vec!["4E00 +VS17"]);
    assert!(state.remap_glyph_names().is_empty());
    assert_eq!(state.uvs_entries[0].glyph_name, "var-a");
}

/// An `exists` above a `map` unrolls the line once per matched name, with the
/// code point computed from the match; the specimen has to read that the way
/// the build does, or a source that maps thousands of han glyphs that way
/// (`exists han-([0-9a-f]{4,5})` / `map U+($1) = han-($1)`) shows none of them.
#[test]
fn lists_characters_an_exists_scoped_map_declares() {
    let d = doc("\
meta height 16
glyph han-4e00 1 1
@@
glyph han-4e01-k 1 1
@@
exists han-([0-9a-f]{4,5})
map U+($1) = han-($1)
exists han-([0-9a-f]{4,5})-k
map U+($1) U+E01E7 = han-($1)-k
");
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &HashMap::new(),
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    assert_eq!(
        state.declared.get(&0x4e00).map(|(n, _)| n.as_str()),
        Some("han-4e00")
    );
    // The variation sequence the second search states, on the base it names.
    assert_eq!(
        state
            .uvs
            .get(&0x4e01)
            .and_then(|m| m.get(&0xE01E7))
            .map(|(n, _)| n.as_str()),
        Some("han-4e01-k")
    );
    // The `exists` lines themselves declare no character.
    assert_eq!(state.declared.len(), 1);
}

/// The grid's own error tint: a character the source maps but the font has no
/// glyph for. No [`crate::glyph_flags`] entry can say this — a flag is per
/// glyph *name*, and the name here stands for nothing — so the cell asks the
/// gid map directly.
#[test]
fn a_character_with_no_glyph_is_flagged_on_the_grid() {
    let d = doc("\
meta height 16
glyph sq 1 1
@@
map U+0061 = sq
map U+0062 = missing
map U+0063 = also-missing sq
");
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let gids: HashMap<String, u16> = [("sq".to_string(), 1u16)].into_iter().collect();
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &gids,
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    state.rebuild_sections();
    let flag_of = |cp: u32| {
        let i = state
            .entries
            .iter()
            .position(|e| e.cp == cp)
            .expect("character is on the grid");
        state.flag_for(Item::Char(i))
    };
    assert_eq!(flag_of(0x61), None);
    assert_eq!(flag_of(0x62), Some(GlyphFlag::Error));
    // The second alternative is there, so nothing is wrong with this one.
    assert_eq!(flag_of(0x63), None);
    assert_eq!(
        state.declared.get(&0x63).map(|(n, _)| n.as_str()),
        Some("sq")
    );
    // A cell that matched nothing still names what the author wrote first, so
    // the status bar and a click have somewhere to go.
    assert_eq!(
        state.declared.get(&0x62).map(|(n, _)| n.as_str()),
        Some("missing")
    );
}

/// Nothing is faulted before there is a font to fault against: an empty gid map
/// is a build that has not landed, not a font with no glyphs.
#[test]
fn no_font_yet_tints_nothing() {
    let d = doc("meta height 16\nglyph sq 1 1\n@@\nmap U+0061 = missing\n");
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &HashMap::new(),
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    state.rebuild_sections();
    assert_eq!(state.flag_for(Item::Char(0)), None);
}

/// The empty target says the character is simply not in the font, so the grid
/// has no cell for it — nothing to tint, and nothing to click.
#[test]
fn an_empty_last_target_leaves_no_cell() {
    let d = doc("\
meta height 16
glyph sq 1 1
@@
map U+0061 = sq
map U+0062 = missing ``
map U+0063 = missing
");
    let docs = [&d];
    let name_parts = crate::document::collect_name_parts(&docs);
    let gids: HashMap<String, u16> = [("sq".to_string(), 1u16)].into_iter().collect();
    let mut state = SpecimenState::new();
    state.rebuild_if_needed(
        &docs,
        &name_parts,
        &gids,
        None,
        &GlyphFlags::default(),
        1,
        1,
    );
    state.rebuild_sections();
    let cps: Vec<u32> = state.entries.iter().map(|e| e.cp).collect();
    assert_eq!(cps, vec![0x61, 0x63]);
    // The one that *is* faulted still is, so the cell is missing for the right
    // reason rather than because nothing is checked any more.
    let i = state.entries.iter().position(|e| e.cp == 0x63).unwrap();
    assert_eq!(state.flag_for(Item::Char(i)), Some(GlyphFlag::Error));
}
