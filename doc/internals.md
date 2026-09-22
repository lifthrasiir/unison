# Uniform internals: where a given design is written down

This is the index of the module-level `//!` docs in `src/`, for anyone changing the code. The
docs themselves hold the reasoning; this file only says which module to open for a question. Paths
are relative to `src/`, and a name in parentheses is the item whose own `///` doc carries the
detail. When you record a new invariant next to the code, add a row here.

`CLAUDE.md` is the short form loaded into every editing session and deliberately does not repeat
this table. The user-facing documentation is `reference.md` (the `.unf` format and the command line) and
`editor.md` (the GUI).

## Format and data model

| Topic | Read |
| --- | --- |
| `.unf` syntax: tokens, comments, directives, glyph blocks, the lenient derive | `document_io.rs` |
| What characters a name may contain | `document_io.rs` (`# Names`), `pattern.rs` |
| `@` as a glyph/`ref` name prefix: what it stands for and where the written form is kept | `document/names.rs` (`expand_at_name`), `document_io.rs` |
| `#`/`##`/`###` headings: why they are a comment to every build stage | `document_io.rs` (`# Headings`), `document/mod.rs` (`DocumentItem::Heading`) |
| `\|\| TEXT` continuation lines: why a keyword and not a mid-line escape, what "the shared whitespace" removes | `document_io.rs` (`# Continuation lines`, `dedent_continuations`) |
| Why a continuation is never tokenized, and the round trip that rests on one dedented line starting at column 0 | `document_io.rs` (`tokenize_strict`), `document/serialize.rs` (`sample_lines`) |
| Which edits skip a rebuild, and why the gate is not "does the font read it" | `document/mod.rs` (`same_for_rebuild`) |
| Why a line the grammar cannot read does not fail the derive | `document_io.rs` (`derive_document`) |
| The declared box (`origin C R` / `extent W H`): the rectangle a glyph claims, and why ink may leave it | `document_io.rs` (`# Glyph blocks`), `document/glyph.rs` (`declared_origin`, `declared_extent`) |
| `advance W` vs `extent W H`: why the width is a flag of its own, and why writing both is an error | `document/glyph.rs` (`GlyphBody::declared_extent`), `document_io.rs` (`parse_glyph_flag_parts_impl`) |
| Why an unstated advance follows the raster and not the grid, and the one accessor `hmtx` and the editor agree through | `document/glyph.rs` (`GlyphBody::stated_advance`) |
| Why an unstated box dimension is the raster's far edge, so an origin is a bearing rather than a shift of the whole box | `document/glyph.rs` (`declared_extent`), `render/ttf_builder/collect.rs` (`resolve_glyph_metrics`) |
| The origin in the grid vs the side bearings it exports as, and why only one is written | `document/glyph.rs` (`GlyphBody::declared_origin`), `render/ttf_builder/collect.rs` (`resolve_glyph_metrics`) |
| `map BASE SELECTOR`: a variation sequence, its two written forms and why length stops at 2 | `document_io.rs`, `document/mod.rs` (`Map::selector`) |
| `map CHAR = A B C`: ordered alternatives, why the choice is per codepoint, `.notdef` as the implicit last one, and the empty target | `render/ttf_builder/expand.rs` (`resolve_map_alternatives`), `issues/maps.rs` |
| Expanding a `map` line's alternatives together, and the memo that parses a wide character spec once | `render/ttf_builder/expand.rs` (`WideMapRows`, `AltTarget`, `map_char_pattern`) |
| Why the lines that write one character spec are settled together, and on every core | `render/ttf_builder/expand.rs` (`settle_wide_groups`, `SettledAlt`, `resolve_map_alternatives`), `parallel.rs` |
| Why one search-scoped `map`'s expansion is indexed rather than searched for | `issues/mod.rs` (`scoped_map_expansions`, `Cx::source_items`) |
| Why a `map` target nothing declares is never a reachability root | `issues/unused.rs` (`GlyphGraph::knows`), `render/ttf_builder/expand.rs` (`MapAlternativeIndex`) |
| Why the duplicate-codepoint table holds two integers per codepoint | `issues/maps.rs` (`MapSite`, `SliceTable`) |
| Which half of a variation sequence may be a range, and why not both | `render/ttf_builder/expand.rs` (`expand_uvs_map_triples`) |
| `meta` keys, name-record derivation, single-assignment rule, the `@LANG` slot | `meta.rs` |
| Faces, slices, the base slice, and why there is no override | `faces.rs` |
| `--output` path rules (`%`, `.ttc`, `.woff2`) | `faces.rs` (`plan_output`) |
| Why a rule about the source is `audit` and not `meta` | `audit.rs` |
| The one `audit` key whose value is a path, and why it is read relative to its own file | `audit.rs` (`AuditEntry::RefImagePath`, `ref_image_root`) |
| `sample LABEL [SUBLABEL] [: MODE]`: the two levels, why a label may carry a text itself, what a mode says | `samples.rs` |
| `sample … : matrix`: the lines read as axes, and why the product is expanded by each consumer | `samples.rs` (`SampleMode`, `SampleText::expanded`), `render/demo/demo.js` (`sampleText`) |
| `sample … : udhr-article1` / `subdivision-flags`: a text the build assembles from `-d`, and why one of them is a whole group | `samples.rs` (`SampleMode::is_generated`, `is_group`), `render/demo/mod.rs` (`collect_samples`), `render/sample.rs` (`udhr_selection`, `subdivision_flags_text`) |
| `prop`: naming Private Use characters, which ones exist, and how `prop block` overrides the UCD | `ucd.rs` (`CharProps`, `CharProps::is_assigned`, `BlockMap`) |
| The `{gc=… ccc=… eaw=…}` group after a character name, and the pinned UCD version | `ucd.rs` |
| Sub-pixel shape codes, `PX_CUSTOM`, and the three questions asked of a cell | `pixel.rs` |
| `$$`, the blank that is not `..`, and why it is an id rather than a spare bit combination | `pixel.rs` (`PX_HARDBLANK`) |
| A hardblank is a claim, not geometry: how claims and ink combine, why only a claim cancels a claim | `pixel.rs` (`blank_op`) |
| Why a rescale carries a claim (and a bare ink flag) by hand, beside the geometry sweep | `document/pixel_grid.rs` (`PixelGrid::rescale`) |
| Snapping an exact region back onto the catalog (a grid on its way into a file) | `detail.rs` (`nearest_shape`), `document/pixel_grid.rs` (`snap_details_to_catalog`) |
| Why the exact sweep carries no rational arithmetic, and the width budget that bounds it | `detail.rs` (`Frac`, `MAX_SWEEP_COORD`) |

## Names, patterns, searches, aliases

| Topic | Read |
| --- | --- |
| Name pattern grammar and its per-context parses | `pattern.rs` |
| `$-N` back-references, and why only a written `(...)` captures | `pattern.rs` (`capture_groups`, `substitute_captures`) |
| Which item binds a back-reference and how far it reaches | `document/name_parts.rs` (`expand_glyph_block`), `alias.rs` (`expand_alias`), `render/ttf_builder/expand.rs` (`map_char_pattern`) |
| Why several groups combine by the largest (not the LCM), and what a ragged group warns | `pattern.rs`, `issues/patterns.rs` (`check_ragged_patterns`) |
| Why a name that is nothing but one `($-N)` is never ragged | `issues/patterns.rs` (`whole_back_reference`) |
| Stating one line for several slices (`map wide\|narrow :`) and per-slice `name-parts` | `document/name_parts.rs` (`SliceNameParts`), `pattern.rs` |
| `exists PATTERN`: what is searched (aliases yes, on-demand no), the one-line scope, one run per match, the fixpoint and its cycle budget, the regex subset | `exists.rs` |
| Why the search's fixpoint scans only what a round added, and the literal prefix that rejects a name before the regex does | `exists.rs` (`resolve_scopes`, `literal_prefix`) |
| A `glyph … = …` under an `exists` | `exists.rs` (`resolve_scopes`), `alias.rs` (`collect_inner`) |
| A code point computed from a match (`U+[BASE+]($N)`) | `exists.rs` (`eval_codepoint`) |
| Where a scoped item is expanded, and why a source-side check reads a scoped `map`'s output but a scoped block's own line | `render/ttf_builder/expand.rs` (`expand_inner`), `issues/mod.rs` (`Cx::source_items`) |
| Why two matched names of one glyph are not an error, and where an indistinguishable pair is caught | `exists.rs` (`# What is searched`), `issues/remap.rs` (the duplicate scan) |
| `glyph A = B`: one glyph id, two names; where each stage canonicalizes | `alias.rs` |
| Why an IDC component keeps its written name past canonicalization | `alias.rs`, `compose.rs` (`expand_compose`) |
| Several names of one pattern block turning out to be one glyph; why two blocks never merge; why a merge is decided on names; which glyph a `remap` stops from merging; `keep` as the opt-out | `merge.rs` |
| Why a scoped block's matches are still one merge candidate set | `merge.rs` (`collect_blocks`) |
| What a pattern glyph block shares with every name it declares | `document/name_parts.rs` (`expand_glyph_block`) |
| What a `$-N` or a `($N)` draws on the grid, and why it is the first expansion | `editor/item_bindings.rs`, `exists.rs` (`FirstMatches`) |

## Composition: refs, anchors, IDC lines, on-demand shapes

| Topic | Read |
| --- | --- |
| Anchor exposure is opt-in; a negative `ref` offset is a bearing | `ref_composite/mod.rs` |
| Grid coordinates vs box coordinates, and the one conversion between them | `ref_composite/anchors.rs` (`rebase_offsets_to_box`), `ref_composite/composite.rs` (`ref_effective_offset_scaled`) |
| Everyone who places a `ref` and so owes that conversion | `render/ttf_builder/contours.rs` (`placed_at`), `editor/backref_shadow.rs`, `editor/document_view/changes.rs` (`inline_ref_to_pixels`) |
| Why the anchor shadow is the one placement with no box term in it | `editor/anchor_shadow.rs`, `ref_composite/anchors.rs` (`derive_ref_offsets_detailed`) |
| An anchor's range: the drawing it selects and the point it reduces to, and why only the class states `align` | `document/glyph.rs` (`AnchorAlign`), `render/ttf_builder/gpos.rs` (`anchor_font_units`) |
| Why a centred anchor class needs its two sizes to share a parity | `issues/anchors.rs` (`check_centred_anchor_parity`) |
| A base offering several slot sizes, and the first-fit that picks one | `render/ttf_builder/gpos.rs` (`slot_holds`, `CcmpKey`), `collect.rs` (the base-alt reachability loop) |
| Why a precomposed mark and a shaped one attach by the same rule, and the exact-before-holds tier only the composite has | `ref_composite/anchors.rs` (`Fit`, `aligned_delta`), `render/ttf_builder/gpos.rs` (`slot_holds`) |
| Why an anchor error drops the glyph (and its cmap entry), like a missing ref | `render/glyph_cache.rs` (`resolve_pending`) |
| Why a glyph the build drops silently is still accounted for, and the test that pins it | `issues/anchors.rs` (`check_anchor_derivation`), `issues/issues_tests.rs` |
| `⿰⿱⿲⿳`: the split, the gap term, why the offsets are derived rather than written | `compose.rs` |
| Why a split fills the box and not the grid, and so moves with a declared `origin` | `compose.rs` (`Raster`, `expand_compose`) |
| `assume ⿰ …`: why an assumed line drops its clearance chores and nothing else, and the one reader of the keyword | `compose.rs` (`# An assumed line`, `IdcOp::of_line`) |
| `⿴⿵⿶⿷⿸⿹⿺⿼⿽`: which sides an enclosure fills, and why its two numbers are offsets rather than gaps | `compose.rs` (`Walls`, `expand_enclosure`) |
| The four boundaries of one line, and the one type every gap is measured between | `compose.rs` (`InkLine`, `Face`, `GapSide`) |
| Which run of a line is the wall a cavity sees | `compose.rs` (`WallFace`) |
| Why an enclosure's clearance total is per axis | `compose.rs` (`Clearance::horizontal`, `report_clearances`) |
| `:WxH.NxM`: the cavity a name promises, why it is a lower bound, and where it may sit | `compose.rs` (`VariantSpec::inner`, `cavity_fits`, `enclosure_rank`) |
| The `:WxH-l` variant name rule, the position tie-break, and why a three-part split's middle slot claims no position | `compose.rs` (`VariantSpec`, `direction_rank`, `IdcOp::slot_direction`) |
| An IDC line written as a pattern, and why its layout is still solved per glyph | `compose.rs`, `document/name_parts.rs` (`expand_glyph_block`) |
| Clearance: the ink a split leaves between its parts and the box, and why the per-part range and the total are both needed | `compose.rs` (`InkProfile`, `measure_clearances`) |
| `audit ideal-clearance`: the prefix match, which rule wins, why an enclosure may have a band of its own | `audit.rs` (`IdealClearances`, `ClearanceBand`) |
| `audit max-contact-run`: how far two parts may run together, why that is a clearance rather than a complaint of its own, why a contact needs no hardblank term, and why it is measured between contours and not cells | `audit.rs` (`MaxContactRuns`), `compose.rs` (`contact_run`, `Face::ink`, `EdgeCover`), `detail.rs` (`DetailRegion::edge_coverage`) |
| Which parts a clearance check can measure, and what it costs a source with no rule | `render/ttf_builder/expand.rs` (`ink_profiles`) |
| Measuring a part that is itself a composite or itself IDC-split, and the walk the check and the fixer share | `ref_composite/mod.rs` (`resolve_reachable`, `derive_compose_body`), `render/ttf_builder/expand.rs` (`ink_profiles`), `fix/clearance.rs` (`Inventory::flatten_composites`) |
| Why a clearance is measured over the declared box | `compose.rs` (`InkProfile::of`) |
| Why an IDC line becomes `ref`s at expansion time, and why the parts are sized by what they declare | `render/ttf_builder/expand.rs` (`expand_compose_lines`), `ref_composite/mod.rs` (`declared_box`) |
| Why an IDC line with an unpicked variant is a TODO and not an error, and what else it silences | `compose.rs` (`expand_compose`, `is_undecided`), `render/ttf_builder/expand.rs` (`expand_compose_lines`) |
| On-demand glyph names, `BitmapFill`, circles, polygons, shears, normalization, the two lattices | `on_demand.rs` |
| Which on-demand grids are remembered between builds | `on_demand.rs` (`make_on_demand_grid`) |
| Why the view synthesizes an on-demand ref instead of waiting for the resolve | `ref_composite/mod.rs` (`resolve_ref_name_for_view`) |
| Inlining a `ref` one level vs. flattening it to pixels, and the same two commands on an IDC line | `editor/document_view/changes.rs` (`inline_ref_once`, `inline_target_at_line`, `inline_compose_once`), `ref_composite/mod.rs` (`InlineSource`) |
| A `ref` to a coloured glyph: which colours travel up, what a `fill` claims | `render/ttf_builder/collect.rs` (`ColorPiece`, `color_pieces_for_body`) |
| The same colours in the editor's live composite | `ref_composite/composite.rs` (`layer_cell_colors`, `target_draws_color`) |
| The flattened grid a composite hands its parent, why it is unioned, and the union's fast paths | `render/ttf_builder/contours.rs` (`from_components_inner`), `document/pixel_grid.rs` (`PixelGrid::blit`) |

## The fixer

| Topic | Read |
| --- | --- |
| What a `uniform fix` command may rewrite, and the two frontends that apply one | `fix/mod.rs` |
| Optimizing clearance: the variant search, the score, why the gaps are arithmetic; which of several equal layouts is chosen | `fix/clearance.rs` (`optimize_clearance`, `arrange`, `Key`) |
| A line the check errors on, planned like a TODO, and the extent an erroring name is still trusted for | `fix/clearance.rs` (`SlotState`, `Key::asked`) |
| A pattern line: the gaps its glyphs share, why the count of warning glyphs comes before the score, which label is the family's to choose, and appending a label to what the line writes | `fix/clearance.rs` (`optimize_pattern_line`, `slot_choices`, `write_pattern_line`) |
| Why an enclosure's placements are searched where a split's gaps are solved; a pattern enclosure line | `fix/clearance.rs` (`optimize_enclosure_line`, `optimize_pattern_enclosure_line`) |
| Which parts the variant search knows about, pattern-declared blocks included | `fix/clearance.rs` (`Inventory::collect`, `block_names`), `alias.rs` (`collect_with_merges`) |
| A fix in the editor: one undo entry per file, nothing written to disk | `app/fix.rs` |

## Diagnostics

| Topic | Read |
| --- | --- |
| What each severity means, and which of them a build, `uniform test` and CI may ignore | `issues/mod.rs` (`Severity`) |
| Why a `chore` is a warning a build counts rather than prints, and what `--chores` does | `issues/mod.rs` (`Severity`), `main.rs` (`report_issues`) |
| Which glyph a finding is about, why a composite carries its components' findings, and why a flag carries the glyph it started at | `glyph_flags.rs` |
| When a finding faults one expansion of a pattern rather than the whole line | `resolve.rs` (`Diagnostic::glyph`), `glyph_flags.rs` |
| A `remap` rule the lookup silently drops, and where that is reported | `render/ttf_builder/gsub.rs` (`shadowed_single_subst_rules`, `build_single_subst_from_pairs`) |
| What a `vectoronly` exemption costs a component shared with an unflagged glyph | `issues/flags.rs` |
| Why validation reads the union face and not the primary; the one check that is still per face | `faces.rs` (`FaceSet::union`), `issues/mod.rs`, `issues/maps.rs` (`uvs_collision_diagnostics`) |
| Who shares the one expansion — the build, validation, the demo page | `main.rs` (the `build` thread scope), `resolve.rs` (`Resolution`), `render/ttf_builder/mod.rs` (`build_faces_from`), `render/sample.rs` (`collect_sample_data_with`) |
| The Issues tab's per-severity filter, and why the editor's line highlights obey it | `app/panels.rs` (`IssueFilter`, `refresh_issue_marks`), `editor/issue_marks.rs` |

## The font build

| Topic | Read |
| --- | --- |
| Writing a TTC, and what the faces of one share | `render/ttf_builder/collection.rs` |
| Why the glyph order is face-independent; why a glyph's GID is its index and how `.notdef` gets to 0 | `render/ttf_builder/collect.rs`, `mod.rs` (`build_faces`, `NOTDEF`) |
| A component glyph nothing maps: why the build synthesizes one with the same declared box | `render/ttf_builder/collect.rs` (the component-extras loop, `resolve_glyph_metrics`) |
| `ulUnicodeRange`/`ulCodePageRange` from the cmap; `gasp` derived, not declared | `render/ttf_builder/os2_ranges.rs`, `tables.rs` |
| cmap format 14, the Default/Non-default split, the GSUB fallback lookup, and where the two can disagree | `render/ttf_builder/tables.rs` (`add_uvs_subtable`), `gsub.rs` (`build_uvs_fallback_lookup`), `issues/maps.rs` (`uvs_collision_diagnostics`) |
| Why a selector needs a plain cmap entry, and why its synthesized glyph's name is unwritable | `render/ttf_builder/collect.rs`, `mod.rs` (`vs_glyph_name`) |
| Feature targets, `DFLT`/LangSys fallback; a remap group is one lookup and rule order is match priority | `render/ttf_builder/gsub.rs` |
| Why one LangSys names a feature tag only once | `render/ttf_builder/gpos.rs` (`merge_anchor_feature_lookups`) |
| Lookup order, `remap group` and its stable toposort | `document/remap.rs` (`remap_group_order`) |
| `assert shape` and why `@lang` is BCP 47; what a test run builds, and how the editor's stays fast | `render/assert.rs` (`run_assertions_inner`), `app/background.rs` (`run_shape_assertions`) |
| Contour coordinate spaces (normalized vs `_at`) | `render/contour.rs` |
| `desync`, `vectoronly` and the closure over the `ref` graph; why a `vectoronly` half of a color/mono pair exempts only its own layers | `render/ttf_builder/mod.rs`, `collect.rs` (`vectoronly_closure`), `document/glyph.rs` (`vectoronly_layers`, `vectoronly_covers`) |
| `meta bitmap-axis`: both drawings in one font, the three tables that carry them, and why the axis is a switch | `render/ttf_builder/mod.rs`, `meta.rs` (`MetaEntry::BitmapAxis`), `tables.rs` (`BITMAP_AXIS_TENT_START`, `full_deltas`) |
| Making the two builds' outlines point-compatible | `render/ttf_builder/masters.rs` |
| How a colour glyph varies: per COLR layer, paired by source | `render/ttf_builder/masters.rs` (`variations_for`, `Variations`), `mod.rs` (`CollectedColorLayer::source`) |
| Why the editor's preview keeps two static faces where the demo took the axis | `render/ttf_builder/mod.rs` (`build_face_variable`), `app/background.rs` |
| Why an expansion is face-independent, and where a face is applied to it instead | `faces.rs` (`FaceSet::union`), `render/ttf_builder/collect.rs` (`face_items`) |
| Why only the union face is traced, and what a secondary face costs instead | `render/ttf_builder/mod.rs` (`build_faces_from`), `collect.rs` (`collect_face_cmap`) |
| Why every map here is hashed with `rustc-hash` rather than `std`'s default, and what that costs | `hash.rs` |
| Hashing a pixel grid for a cache key: why the cells go in as one write | `document/pixel_grid.rs` (`hash_cells_into`), `render/ttf_builder/contours.rs` (`hash_grid_for_cache`), `ref_composite/mod.rs` (`hash_grid_into`) |
| Why a glyph's own grid is cached under the grid as written, why a composite key reads a component's stored hash, and why cache entries share their contours | `render/ttf_builder/contours.rs` (`trace_own_grid`, `CachedContours::grid_hash`, `CachedContours::contours`) |
| Which build stages run at once, and what they must not share | `render/ttf_builder/mod.rs` (`build_faces`, `build_font_pair_cached_for`), `contours.rs` (`ContourCaches`) |
| Why a resolution round is a wave; splitting a memo off its tracer | `render/glyph_cache.rs` (`resolve_pending`, `CompositeBuilder`), `ref_composite/mod.rs` (`resolve_expansion_cached`), `render/ttf_builder/contours.rs` (`ContourBuilder`) |
| Dropping a composite that can never resolve before the expensive loop sees it | `render/glyph_cache.rs` (`drop_unresolvable`) |
| Why a resolve recomposes only what an edit reached | `ref_composite/mod.rs` (`CompositeGridCache`) |
| Which of a `build`'s outputs are produced at once | `main.rs` (`OutputWork`) |
| Why the directory load reads its files on many threads; what a refresh re-reads | `render/ttf_builder/mod.rs` (`load_docs_from_directory_with_sources`, `DirCache`) |
| Typing a glyph no `map` names: the cascade, why a per-glyph cost cannot answer it, the repair search | `render/reach.rs` (`Cascade`), `render/ttf_builder/collect.rs` (`remap_only_sequences`) |
| Folding a secondary `face` into the demo page's font: why a stylistic set, why last, what it cannot carry | `render/ttf_builder/fold.rs` (`fold_secondary_faces`, `allocate_feature_tags`, `FoldDelta`), `render/demo/mod.rs` (`DemoFace::unmapped`) |

## The demo page and the specimen

| Topic | Read |
| --- | --- |
| What `demo.html` embeds, and the four ways its specimen differs from the editor's | `render/demo/mod.rs` |
| Why the demo font traces the union face where it shows the primary one's cmap | `render/ttf_builder/mod.rs` (`build_face_variable`) |
| Switching face on the page; why the sample panel asks for it explicitly | `render/demo/mod.rs` (`face_rules`), `demo.js` (`setFace`, `faceMissing`), `demo.css` (`.s-text`) |
| The corner triangle a cell wears when it begins a sequence, and its detail row | `render/demo/demo.js` (`toggleDetail`, `seqLabel`, `sizeChunk`), `demo.css` (`.cell .mk`, `.row.detail .cell .n`) |
| What the page is told about sequences, and why no glyph name is among it | `render/demo/mod.rs` (`collect_sequences`) |
| What the blob is modelled into: delta-coded runs, front-coded names, one entry per naming rule | `render/demo/mod.rs` (`DemoBlock::runs`, `DemoNames`, `widen_runs`, `collect_names`), `demo.js` (`nameOf`) |
| Lazy chunks, and folding a long block in the middle | `render/demo/demo.js` (`FOLD_OVER`, `foldMarker`), `render/demo/mod.rs` |
| The same fold in the editor's specimen | `specimen.rs` (`FOLD_EDGE_ROWS`, `Row::Fold`, `SpecimenState::unfolded`) |
| The dotted circle in a demo cell, and why `hmtx` decides which cells get one | `render/demo/mod.rs` (`CELL_ZERO_ADVANCE`, `zero_advance_codepoints`), `demo.css` (`.dc`) |
| Why a demo cell is bidi-isolated; which contrast the tokens are held to | `render/demo/demo.css` (`.cell`, `:root`) |
| The sample panel: where a typed text is kept, how a UDHR key becomes a language name, the one size for both drawings | `render/demo/mod.rs` (`# The sample panel`), `demo.js` (`selectSample`, `ssSet`, `langName`, `state.em`, `snapZoom`), `render/sample.rs` (`udhr_selection`, `UdhrEntry`) |
| Why a character the font cannot draw is tinted by the specimen and not by a glyph flag | `specimen.rs` (`CharEntry::unresolved`, `flag_for`) |
| The specimen's warning/error tints, and why a click on a tinted cell lands on the component | `specimen.rs` (`flag_bg`, `goto_target`), `glyph_flags.rs` (`GlyphFlags::source`) |
| Why what the specimen reads is collected in the background; which remap targets it remembers; why it is handed the searches and aliases; the one clone per line | `specimen.rs` (`SpecimenData`, `remap_targets`, `rebuild_if_needed`), `app/background.rs`, `exists.rs` (`Scope::rebind`) |
| The specimen's three options, its cache keys, and filling a block out | `specimen.rs` (`SpecimenOptions`, `SpecimenState::cached_gen`) |
| Where a variation sequence sits on the specimen | `specimen.rs` (`UvsEntry`, `uvs_label`, `uvs_boundary`) |

## The editor

| Topic | Read |
| --- | --- |
| The editor as a widget; what is per-instance vs per-pane | `editor/mod.rs`, `editor/ids.rs` |
| Split panes, their invariants and key chords | `app/panes.rs` |
| Go back / go forward; where a jump leaves its target and why going back restores a page | `app/history.rs` (`NavLoc::view_offset`), `editor/mod.rs` (`ScrollIntent`) |
| `ref … goto`: a jump carried on to the drawing, why one gesture leaves two history entries, and why the redirect is read off the source rather than the resolve | `app/goto_redirect.rs` (`follow_goto_chain`, `goto_redirect`, `redirect_at_declaration`), `app/mod.rs` (`record_nav_chain`), `exists.rs` (`template_captures`), `document/glyph.rs` (`GlyphRef::goto`) |
| The Search pane; why a search lists a name written as a pattern; why a click reads no files | `app/search.rs` (`pattern_denotes`, `may_write_a_pattern`), `app/docs.rs` (`FontSource`) |
| What a search kind is and what adding one costs; the Ctrl/Cmd+F cycle and why the box's focus is recorded | `app/search.rs` (`SearchKind`, `SearchState`, `focus_search_box`) |
| A menu entry as a value, what adding one costs, and why a command requests rather than performs | `app/commands.rs` (`Command`, `CommandCx`) |
| The palette (Ctrl/Cmd+P): what it lists and caches, subsequence and code point matching, why it takes the list keys out of the queue, where the keyboard goes back to, and why a command waits a frame | `app/palette.rs` (`narrow`, `codepoint_fit`, `take_list_keys`, `PaletteState`) |
| One jump to a name for a link, a specimen click and the palette, and the caret a jump from no link departs from | `app/mod.rs` (`jump_to_name`, `caret_nav_loc`) |
| Why both ends of a search divide a file the same way, and what goes wrong when they do not | `app/search.rs` (module note, `LineCarry`), `document_io.rs` (`walk_source_lines`) |
| Why opening a file from the snapshot keeps its generations | `app/docs.rs` (`open_document_from_text`) |
| Which tokens on a line name what | `editor/line_fields.rs` |
| Where a Ctrl/Cmd+click on a `$-N`/`($N)` goes; a glyph name in a `//` comment; Ctrl/Cmd+`]` | `editor/doc_links.rs` (`find_capture_target`, `extract_comment_links`), `editor/document_view/paint.rs` (`link_at_caret`) |
| Ctrl/Cmd+click on a reference written as a *pattern*: expanding it, grouping the expansions by where they go, when the reader is asked to pick and when the click just jumps | `app/goto_pattern.rs` (`resolve`, `block_captures_at_line`, `expand_link_token`), `editor/goto_popup.rs`, `editor/document_view/mod.rs` (`NavTarget::Pattern`) |
| The walk a list popup offers — the selection, the window it is shown through, and the keys that move it — shared by completion, the goto choice and the palette | `editor/list_popup.rs` (`ListNav`, `read_move`, `read_typed_list_key`, `show_window`) |
| Typing a character by code point (Ctrl+K), and why not Alt; why a click on the popup's own chrome does not dismiss it | `editor/codepoint_popup.rs` (`resolve_field`) |
| Completion: the `:` prefix rule, which item a listing starts on, typing on from a walked item, which keys it claims, what is never offered, what an IDC slot's listing drops | `editor/autocomplete.rs` (`effective_prefix`, `filter_candidates`, `select_for_text`, `continue_from_selection`, `handle_keys`, `collect_candidates`), `compose.rs` (`direction_rank`) |
| Folding: what a group is, why the list rides on `edit_gen`, which blocks start folded, where the caret goes, closing vs opening scroll | `editor/folding.rs` (`fold_groups`, `apply_initial`, `toggle_at`, `snap_caret`, `FoldScroll`) |
| The gutter's marker columns; why wrapping is measured against the widest gutter | `document_view/layout.rs` (`GutterLayout`, `page_has_fold_marker`), `document_view/mod.rs` (`wrap_width`), `editor/folding.rs` (`nesting_depth`) |
| Why a heading draws in zoom steps and re-picks the face | `document_view/layout.rs` (`heading_font_size`, `heading_font`), `app/mod.rs` (`uniform_family_at_size`), `editor/minimap.rs` |
| Why the minimap is not the document to scale, and how a click, the wheel and the viewport box are placed on it | `editor/minimap.rs` (module note, `MinimapMap`, `pointer_scroll_target`, `strip_scroll`) |
| Ctrl/Cmd+`/`: which lines a toggle takes, and the grid it demotes and promotes | `editor/comment.rs` |
| A header and its grid are one block: Enter, line-wise copy/cut, paste | `editor/editing.rs` (`insert_newline`), `editor/doc_input.rs` (`current_line_range`, `paste_text`) |
| Why an edit on a header or `ref` line waits before it reparses | `editor/document_view/changes.rs` (`apply_pending_rederive`) |
| Why a right-click moves the caret first; why a menu action hands the keyboard back | `editor/document_view/paint.rs` (`secondary_pos`, `refocus_after_menu`), `editor/mod.rs` (`refocus`) |
| The two rectangles a resize drags (box vs canvas), which flags and which `ref`s it writes, units | `editor/glyph_resize.rs` (`canvas_box`, `CanvasStart`, `boxed_for`), `app/resize.rs`, `document_io.rs` (`replace_glyph_box_flags`), `ref_composite/anchors.rs` (`DeriveOutcome::anchor_placed`) |
| A resize preview: uncommitted text, and everything that has to drop it | `editor/glyph_resize.rs` (`cancel`), `app/docs.rs` (`flush_pending_changes`) |
| A floating pixel selection: what commits it, and who lands it before reading the buffer | `editor/pixel_selection.rs` (`reconcile`, `effective_selection`, `select_all`), `app/docs.rs` (`commit_floating_selection`), `editor/mod.rs` (`pixel_select_anchor`) |
| The empty band below the last line | `editor/document_view/paint.rs` (`paint_document_area`) |
| Who owns a key while an IME is composing | `editor/doc_input.rs` (`ImeKeyGuard`) |
| A reported line's tint and the message drawn after it; why the message is painted rather than annotated | `editor/issue_marks.rs`, `editor/document_view/paint.rs` (`paint_document_area`), `editor/colors.rs` (`issue_colors`) |
| How a finding follows lines opened or deleted after its build, and why a line is located by id rather than index | `editor/issue_marks.rs` (`# Following edits`), `document/line_id.rs` |
| Which line an edit that rebuilds lines from text continues, and where a snapshot's ids are carried over (open, reload, refresh) | `document/mod.rs` (`DocLine::continue_text_edit`, `DocLine::with_id_of`, `DocLine::adopt_line_ids`), `app/docs.rs` (`open_file`, `apply_reloaded_lines`), `app/watch.rs` (`apply_directory_snapshot`) |
| Inline annotations, and the dotted circle before a zero-advance character | `editor/annotations.rs` (`zero_advance_placeholders`, `paint_dotted_circle`, `display_prefix`), `editor/visual_lines.rs` (`compute_wrap_segments`) |
| Alt + wheel / Alt + Up/Down over a number | `editor/document_view/number_scroll.rs` |
| The metrics overlay | `editor/grid_render.rs`, `editor/document_view/layout.rs` (`GlyphMetrics`) |
| The shape palette: rotation orbits, rotation as separate state | `editor/glyph_widget.rs` |
| The anchor shadow, the backreference shadow, and what they share | `editor/anchor_shadow.rs`, `editor/backref_shadow.rs`, `editor/shadow.rs`, `editor/mod.rs` (`EditMode::PixelSelect`) |
| The reference chart strip above a `glyph` line: where the strips are, why one fixed row height | `editor/ref_images.rs` (`REF_IMAGE_ROW`, `RefImages::end_frame`), `audit.rs` (`ref_image_root`) |
| Files changed outside the editor; F5; the poll backend | `app/watch.rs` (`request_refresh`, `run_scan`, `apply_directory_snapshot`, `poll_snapshot`, `next_poll_delay`) |
| Saving off the UI thread, the revision a write is credited to, why quitting waits, why the bytes are recorded first | `app/save.rs`, `editor/undo.rs` (`SavePoint`), `app/docs.rs` (`knows_disk_bytes`, `confirm_close_and_maybe_save`) |
| Rebuild debouncing, generations, one build at a time, cancellation, why the font result is sent early | `app/background.rs` (`UniformApp::rebuild`, `take_current_font_build`, `arm_initial_font_build`, `set_selected_face`), `cancel.rs`, `specimen.rs` |
| Why a rebuild sends the composites ahead of validation, and what the recomposition borrows to run beside it | `app/background.rs` (`# Three results`, `take_derived_data`), `app/mod.rs` (`ResolvedMessage`), `ref_composite/mod.rs` (`resolve_expanded_items_shared`) |
| Which stages notice a cancel, and why the next edit waits for the ones that do not | `issues/mod.rs` (`collect_issues_cancellable`), `render/ttf_builder/expand.rs` (`expand_documents_cancellable`), `main.rs` (`rebuild_like_the_editor`) |
| Why the remembered face is applied before the first build | `app/mod.rs` (`with_settings`) |
| What survives between runs, where the settings file lives | `app/settings.rs`, `main.rs` (`with_app_id`) |
| Where the seconds before the first frame go; what one edit costs; where an edit's wait goes in the running editor | `startup.rs`, `main.rs` (`run_edit_probe`), `app/timing.rs` |
| Why Windows and macOS get a different allocator, and the numbers behind each | `main.rs` (`GLOBAL_ALLOC`) |
| Why the release build compiles some dependencies for size, and which renderer backend it links | `Cargo.toml` (`[profile.release.package."*"]`) |

## The preview

| Topic | Read |
| --- | --- |
| Bidi: why each backend resolves its own levels, and what the shared path still hands it; where the Bidi_Class table comes from | `preview/mod.rs` (`Paragraph`), `preview/bidi.rs`, `Cargo.toml` |
| Mirroring an RTL run | `preview/rustybuzz.rs`, `preview/bidi.rs` |
| Why a backward run's glyph order is normalized rather than trusted to the shaper | `preview/mod.rs` (`to_visual_order`), `preview/directwrite.rs` |
| Visual order against logical indices; the caret's affinity; why an arrow moves visually but Shift+arrow extends logically; the caret's shape | `preview/cluster.rs` (`CaretPos`, `step`), `preview/widget.rs` (`caret_affinity`, `take_visual_step`, `caret_shape`) |
| Forcing the paragraph direction, on the shared path and on Core Text | `preview/widget.rs` (`show_direction_combo`), `preview/bidi.rs` (`ParagraphDirection`), `preview/coretext.rs` (`paragraph_style_for`) |
| Why a character the built font lacks must not keep the glyph id Core Text hands back | `preview/coretext.rs` (`run_is_font`) |
| How tall a preview row is; why the editor's preedit box cannot crop a glyph but the preview's could | `preview/metrics.rs` (`VMetrics`), `editor/document_view/paint.rs` |
| The text-editing keys, and the state both the editor and the preview edit through | `editor/doc_input.rs` (`TextEdit`) |
| Where a `sample` reaches the reader: the demo's panel and the editor's *Use* button | `render/demo/mod.rs` (`collect_samples`), `editor/document_view/paint.rs` (`sample_use_rect`), `app/mod.rs` |
