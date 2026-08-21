# Uniform

Bitmap font editor with sub-pixel shape support and a TTF builder, plus the `font/` sources of the
Unison font itself. egui/eframe GUI, Rust 2024 edition. Single binary `uniform` with four modes:
GUI (default), `build`, `test`, and `fix` (the one that rewrites the source).

**This file is an index.** The reasoning behind each design — the `.unf` format, composition rules,
the editor's structure — lives in the module-level `//!` docs of the code that
implements it; this file says which module that is. Keep it that way: when a new invariant is worth
recording, put it next to the code and add at most a line here.

## Build & Run

```sh
cargo build -r    # normal build
cargo test        # unit + golden + GUI-harness tests
make              # build unison.ttf/.woff2 + sample.html/sample.png/live.html
make test         # the above, the headless test suite, then the `assert` directives in font/
```

Cross-compiling for Windows — use these instead of the plain commands when the current environment
is *not* Windows:

```sh
cargo xb -r       # cargo xwin build --target x86_64-pc-windows-msvc
cargo xrr         # run the compiled release executable (only to be used by users)
cargo xr          # ditto, debug
```

Both run aliases go through `run-local.cmd`, which copies `uniform.exe` + `uniform.pdb` to
`%LOCALAPPDATA%\uniform\<profile>\` and runs *that* copy (working directory unchanged, so relative
arguments still resolve). **Never run the binary from the repo path** — the repo is an SMB mount and a
PE image is demand-paged from its file for the life of the process, so a cold code page that the
share cannot serve kills the process; `run-local.cmd`'s comments have the whole story.

The `build`/`test`/`fix` subcommands require native execution:

```sh
cargo run -r -- build -i font/ -o unison.ttf [-o unison.woff2] [-o unison-%.ttf] [-o unison.ttc] \
    [--sample-html F] [--sample-png F] [--live-html F] [--demo-html F] [-d data] \
    [--woff2-quality fast|max]
cargo run -r -- test -i font/       # run `assert` directives; exit 1 on failure
cargo run -r -- fix -i font/ --optimize-clearance [--dry-run]   # rewrite the source: see `fix/`
cargo run -r -- probe -i font/ [-n 2]   # startup timing with no window: see `startup.rs`
```

Output extension picks the format (`.woff2` → WOFF2, anything else → TTF); `--woff2-quality max`
is for the files that are actually published, and costs about 1.5 s per face (`render::Woff2Quality`).
Both subcommands print parse errors per file and then the full `issues/` validation report (`error:`/`warning:` with
`file:line:`); the font still builds when only warnings/refs-to-nothing exist, so read the report.
A single `error:` (from either the parse or the validation pass) makes both subcommands **exit 1** —
`build` still writes every output file first, so a CI run can publish them and fail afterwards, which
is what `.github/workflows/pages.yml` does with its `report` job.

The GUI takes an optional font-directory argument: `cargo run -r -- font/`.

### Environment variables

| Var | Effect |
| --- | --- |
| `UNIFORM_PERF` | `[perf]` per-stage timing logs: font/derived-data rebuilds (`app/background.rs`), the build pipeline's own stages in every mode including `build` (`startup.rs`, `PerfStage`), plus the startup report on stderr after the first frame |
| `UNIFORM_UPDATE_GOLDEN=1` | Rewrite `testdata/*.golden` instead of comparing (`cargo test golden`) |
| `UNIFORM_WATCH_POLL_MS` | Shortest interval between two re-scans of a font directory on a network volume (default 2000; the interval itself follows what a scan costs — `app/watch.rs`) |
| `UNIFORM_PROFILE_RUNS` | Iteration count for the `ref_composite` profiling test |

Cargo features: `editor` (default) pulls in eframe/egui/tiny-skia/notify/rfd/arboard. `--no-default-features`
builds the headless CLI only — **it breaks easily**, since most code is under `#[cfg(feature = "editor")]`,
and `cargo test` never builds it. It has rotted twice, so `make test` now runs `cargo test
--no-default-features` (the `check-headless` target) rather than trusting anyone to remember.

Which side of the boundary a fix belongs on: an item the headless *binary* genuinely does not need stays
`#[cfg(feature = "editor")]`, and a test that reaches for it is gated the same way — `detail.rs` gates its
own rotation/snapping tests exactly so. Widening a gate to `any(feature = "editor", test)` pulls the item's
whole dependency chain along with it, so it is for items whose callers are already core. An item that is
live in the headless *test* build but dead in the headless *binary* takes
`#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]`, which is `expect` and not `allow`
on purpose: it fails once the item does get used there.

## Source Layout

Core (feature-independent):

- `document/` / `document_io.rs` — the `.unf` data model, parser and serializer. **`document_io.rs`
  is the format reference**: tokens, comments, every directive, glyph blocks and their flags.
  `document/mod.rs` holds `Document`/`DocumentItem`/`DocLine`; what hangs off them is split by what
  it models — `pixel_grid.rs`, `glyph.rs` (refs, anchors, IDC, `GlyphBody`), `names.rs` (`GlyphName`
  and `@`), `name_parts.rs`, `remap.rs`, and `serialize.rs` for the way back to text.
- `alias.rs` — `glyph NAME = TARGET`: a second *name* for a glyph, sharing its glyph id. Holds the
  chain/cycle rules and the list of which pipeline stages canonicalize where.
- `merge.rs` — implicit merging: the names one `glyph` *pattern* block declares that describe the
  same glyph, folded into one glyph id by producing `alias.rs`'s input. Holds why the candidates are
  one block's expansions and never two blocks, the σ fixpoint over the `ref`/IDC graph, why
  why a `remap`'s *inputs* are excluded where its outputs are not, and `keep` as the opt-out.
- `exists.rs` — `exists PATTERN`: the inverse of a name pattern — a search over the names the
  source declares, repeating the next line once per match with `$0`/`$N` bound. Holds what is
  searched (and why on-demand names are not), the one-line scope rule, the fixpoint and its cycle
  budget, and the regex subset. Tests in `exists_tests.rs`.
- `pattern.rs` — `NamePattern`, the single name-expansion engine. The same syntax parses differently
  per context on purpose; its module docs spell the three contexts out.
- `pixel.rs` — `PixelShape`/`PixelGrid`, the shape-code catalog (`PX_*`), boolean ops, `rescale`.
- `detail.rs` — `DetailRegion`: exact per-pixel sub-pixel geometry on a `1/den` lattice, combined by
  an exact trapezoid sweep. This is what makes composition exact instead of code-approximate. The
  sweep's arithmetic is bounded by construction (`MAX_SWEEP_COORD` carries the width budget); read
  `Frac` before adding arithmetic to it.
- `ref_composite/` — composite (`ref`) resolution. `mod.rs`'s docs hold two things nothing else
  records: **anchor exposure is opt-in** and a **negative `ref` offset is a bearing**. `anchors.rs`
  is the offset/anchor derivation those two rules govern, `composite.rs` the layout and flattening.
- `compose.rs` — the `⿰⿱⿲⿳` line: a glyph's box split along one axis, with the offsets *derived*
  from what the parts declare, and the ink the parts leave each other (*clearance*) measured against
  `audit ideal-clearance`. Also the `:WxH-l` variant name rule (size + position) every han part is
  named by. Tests in `compose_tests.rs`.
- `on_demand.rs` — the names nothing defines but that describe a shape (`WxH`, triangles, `-circle`,
  `-polyN`) and the geometry each stands for. Holds the grammar, the `BitmapFill` rule, why polygon
  names are *normalized*, and the two lattices a curve is cut on (`POLY_Q`, `REGION_DEN`) — the
  latter is not freely choosable. Tests in `on_demand_tests.rs`.
- `resolve.rs` — shared vocabulary for the resolution pipeline (`ItemRef` provenance, `Diagnostic`),
  so build/editor/validation cannot drift apart. Resolution emits issues directly.
- `faces.rs` — `face`/`slice`: which typefaces the source describes and what each contains. Holds the
  base-slice invariant (a character whose mapping varies must not be in the base), the face-id rules,
  and `plan_output` — the table of which `--output` path means one file, one per face, or a
  collection. Tests in `faces_tests.rs`.
- `audit.rs` — the `audit` directive: rules the *source* is held to (`audit ideal-clearance han-* 0
  1`, `audit max-contact-run han-* 2`), as opposed to the values the font file carries. Holds why that is not a `meta` key, the
  single-assignment rule and the prefix match. Tests at the bottom of the file.
- `fix/` — `uniform fix`: the commands that rewrite the *source*, and the rules they share (plan
  first, whole lines in place, only what already warns). `clearance.rs` is
  `--optimize-clearance`: the variant search, the score, and why the gaps are solved arithmetically
  rather than searched. Tests in `fix/clearance_tests.rs`.
- `meta.rs` — the `meta` directive: the key set, the `@LANG` language slot, and which font fields are
  *declared*, *derived* and *computed*. Tests in `meta_tests.rs`. Values on the pixel grid are
  declared in pixels and scaled by the builder, like everything else in `.unf`.
- `math.rs` — the one gcd/lcm of the geometry code (binary GCD). A helper more than one module
  wants lives here rather than being re-derived per module.
- `parallel.rs` — `map_indexed`: the one work-stealing loop the build's pure stages (composite
  tracing, composite flattening) run on. Why the work is stolen rather than sliced, and where the
  cancellation check goes, live there.
- `cancel.rs` — `CancelToken`: how a background stage is told its result is no longer wanted, and
  what a cancelled stage is allowed to return. Only the editor cancels; every other caller passes
  `CancelToken::never()`.
- `glyph_flags.rs` — which glyphs the diagnostics report faults, as one tri-state flag per glyph
  (none / warning / error), propagated backwards along the `ref` graph. Holds why attribution is per
  a *line* rather than per expanded glyph, the two paths that can narrow a finding to one expansion
  of a pattern, why `Todo`/`Note` flag nothing, and why a flag carries the glyph it *started* at
  besides the ones it reached. The specimen's cell backgrounds and clicks are the consumer.
- `issues/` — cross-document validation (missing refs, duplicate maps, unused glyphs, remap sanity).
  `mod.rs` is `Severity`, `Issue` and the driver that runs every check over one shared `Cx`; each
  check is a module of its own (`slices`, `glyph_names`, `remap`, `directives`, `maps`, `unused`,
  `anchors`, `colors`, `patterns`).
- `script_run.rs` — script segmentation for shaping, mirroring browser behavior.
- `startup.rs` — the timeline of everything before the first painted frame (loader, directory read,
  initial font build), and the three ways to read it out. Written for the slow-launch-over-SMB
  question; the `probe` subcommand is its headless form.
- `ucd.rs` — the character properties shown beside a character name, and the `prop` directives a
  source states them with (`CharProps`). Nothing in the font depends on them; the status bars, the
  `sample.html` tooltips and the demo page's grid do. Also `BlockMap`: the bundled `Blocks.txt` with
  the source's own `prop block` claims over it. Blocks and assignedness are *not* behind the `editor`
  feature — the headless `build` lays out `demo.html` with them — which is why `icu_properties` is a
  plain dependency rather than an optional one.
- `render/demo/` — `demo.html`: the page the three sample outputs are being folded into. It embeds
  the *font* (both flavors of the primary face) and one JSON blob instead of pre-rendered SVG, and
  `demo.js`/`demo.css` build every cell from them. Holds what the specimen there does differently
  from the editor's and why.
- `render/` — `contour.rs` (pixel shapes → contours; note the normalized vs `_at` coordinate spaces),
  `glyph_cache.rs` (the composite-resolution driver `ttf_builder` and `sample` share), `sample.rs`,
  `assert.rs` (`assert` directives).
- `render/ttf_builder/` — contours → TrueType, GSUB, cmap. `mod.rs` lists the stage submodules;
  `gsub.rs` documents feature targets and OpenType scope fallback. Tests in `render/ttf_tests/`.
- `golden.rs` — `cfg(test)` golden snapshots over `testdata/`.

Editor (feature `editor`):

- `app/` — `UniformApp` eframe entry point. `mod.rs` (the struct and the `eframe::App` loop),
  `background.rs` (the debounced build/derive/assert threads and the generation rules their consumers
  must respect), `docs.rs`, `history.rs` (go back/forward), `menus.rs`, `panels.rs`, `panes.rs` (the
  split-editor model and its two invariants), `search.rs` (the Search pane),
  `rename.rs`, `resize.rs` (carrying a glyph resize across every file that refers to the glyph),
  `fix.rs` (applying a `crate::fix` plan to the open documents, undoably),
  `settings.rs` (what survives between runs, and what egui persists instead),
  `watch.rs` (the OS watch on the font directory and what an external change may do), `toast.rs`,
  `zoom.rs`.
- `editor/mod.rs` — `EditorState`, `EditMode`, and **the editor-is-a-widget model**; `editor/ids.rs`
  is the per-instance `egui` id namespace it rests on.
- `editor/document_view/` — the editor widget: `DocumentEditor::show` and the `show_document` frame
  loop behind it (most churn in the editor). `mod.rs` is the loop and the view cache; `layout.rs`
  (grid extents/strips, the visual-line model, `GlyphMetrics`), `paint.rs`, `scroll.rs`, `keys.rs`,
  `popups.rs`, `changes.rs`.
- `editor/folding.rs` — collapsing a run of lines to its first one: what a group is, why the
  group list rides on `Document::edit_gen`, and why a fold is re-found by its header's text.
- `editor/glyph_resize.rs` — F2 over a grid: dragging a glyph's boundary, and the two directions a
  resize propagates in (its own anchors/refs one way, every `ref` naming it the other). Tests in
  `glyph_resize_tests.rs`; the cross-file half is `app/resize.rs`.
- `editor/` others — `shadow` (`anchor_shadow`/`backref_shadow`), `caret`, `codepoint_popup`, `visual_lines`, `line_fields` (**the single place that
  knows where names live** on a line), `doc_links`, `doc_input`, `editing`, `reconcile`, `undo`,
  `autocomplete`, `annotations`, `colors`, `minimap`, `inline_tools`, `glyph_widget`, `grid_render`
  (grid painting and the metrics overlay), `pixel_interaction`, `pixel_selection`, `harness`,
  `view_tests`.
- `sidebar.rs` — `.unf` file list (open, rename, create). `specimen.rs` — specimen rendering; its
  three cache keys (documents, `SpecimenOptions`, column count) are documented there and are easy to
  get wrong.
- `edit_menu.rs`, `preview/` — bottom live-preview panel: rustybuzz shaping + platform rasterizer
  (`coretext.rs` on macOS, `directwrite.rs` on Windows). `preview/widget.rs` is a multi-line text
  field that runs on the *editor's* text model and key handler; only its layout is its own, and
  `preview/metrics.rs` is the vertical half of that layout — read from the face, not assumed.

`font/*.unf` are the font sources (one file per category). `testdata/` holds test-only `.unf` files
plus goldens. `data/` holds sample-generation inputs (confusables, UDHR text) read at build time
through `-d data`, plus `Blocks-17.0.0.txt`, which is the one file there compiled *into* the binary
(`include_str!` from `ucd.rs`) because the editor needs it with no `-d` in sight.

### Where a given design is written down

| Topic | Read |
| --- | --- |
| `.unf` syntax: tokens, comments, directives, glyph blocks | `document_io.rs` |
| What characters a name may contain | `document_io.rs` (`# Names`), `pattern.rs` |
| `@` as a glyph/`ref` name prefix: what it stands for and where the written form is kept | `document/names.rs` (`expand_at_name`), `document_io.rs` (`# Names`) |
| `map BASE SELECTOR`: a variation sequence, its two written forms and why length stops at 2 | `document_io.rs`, `document/mod.rs` (`Map::selector`) |
| Which half of a variation sequence may be a range, and why not both | `render/ttf_builder/expand.rs` (`expand_uvs_map_triples`) |
| cmap format 14, the Default/Non-default split, and the GSUB fallback lookup behind it | `render/ttf_builder/tables.rs` (`add_uvs_subtable`), `gsub.rs` (`build_uvs_fallback_lookup`) |
| Why a selector needs a plain cmap entry, and why its synthesized glyph's name is unwritable rather than reserved | `render/ttf_builder/collect.rs`, `mod.rs` (`vs_glyph_name`) |
| Where cmap 14 and the fallback lookup can disagree (glyph-keyed vs codepoint-keyed) | `issues/maps.rs` (`uvs_collision_diagnostics`) |
| The declared box (`origin C R` / `extent W H`): the rectangle a glyph claims, and why ink may leave it | `document_io.rs` (`# Glyph blocks`), `document/glyph.rs` (`declared_origin`, `declared_extent`) |
| `advance W` vs `extent W H`: why the width is a flag of its own, and why writing both is an error | `document/glyph.rs` (`GlyphBody::declared_extent`), `document_io.rs` (`parse_glyph_flag_parts_impl`) |
| Why an unstated advance follows the raster and not the grid, and the one accessor that keeps the editor and `hmtx` agreeing | `document/glyph.rs` (`GlyphBody::stated_advance`) |
| Why an unstated box dimension is the raster's *far edge* — so an origin is a bearing rather than a shift of the whole box | `document/glyph.rs` (`declared_extent`), `render/ttf_builder/collect.rs` (`resolve_glyph_metrics`) |
| The origin in the grid vs the side bearings it exports as, and why only one of them is written | `document/glyph.rs` (`GlyphBody::declared_origin`), `render/ttf_builder/collect.rs` (`resolve_glyph_metrics`) |
| Grid coordinates vs box coordinates: which of the two an offset, an anchor and a ref placement are in, and the one conversion between them | `ref_composite/anchors.rs` (`rebase_offsets_to_box`), `ref_composite/composite.rs` (`ref_effective_offset_scaled`) |
| Everyone who places a `ref` and so owes that conversion: the build, the sample, the backreference shadow, flattening | `render/ttf_builder/contours.rs` (`placed_at`), `render/sample.rs` (`placed_at`), `editor/backref_shadow.rs`, `editor/document_view/changes.rs` (`inline_ref_to_pixels`) |
| Why the anchor shadow is the one placement with no box term in it | `editor/anchor_shadow.rs`, `ref_composite/anchors.rs` (`derive_ref_offsets_detailed`) |
| `meta` keys, name-record derivation, single-assignment rule | `meta.rs` |
| Faces, slices, the base slice, and why there is no override | `faces.rs` |
| `--output` path rules (`%`, `.ttc`, `.woff2`) | `faces.rs` (`plan_output`) |
| Writing a TTC, and what the faces of one share | `render/ttf_builder/collection.rs` |
| Why the glyph order is face-independent | `render/ttf_builder/collect.rs`, `build_faces` in `mod.rs` |
| Why a glyph's GID is its index, and how `.notdef` gets to GID 0 | `render/ttf_builder/mod.rs` (`NOTDEF`), `collect.rs` |
| `ulUnicodeRange`/`ulCodePageRange` derivation from the cmap | `render/ttf_builder/os2_ranges.rs` |
| Name pattern grammar and its per-context parses | `pattern.rs` |
| `exists PATTERN`: searching the declared names instead of listing candidates, and what `$0`/`$N` bind | `exists.rs`, `document_io.rs` (`# Directives`) |
| Why an `exists` governs exactly one line, and why it does not stack | `exists.rs` (`# Scope`) |
| Why searches are a fixpoint, and the round budget that stands in for cycle detection | `exists.rs` (`# Recursion`), `resolve_scopes` |
| Which names a search may find — aliases yes, on-demand names no | `exists.rs` (`# What is searched`) |
| Why two matched names of one glyph are not an error, and where a pattern that cannot tell two matches apart is caught instead | `exists.rs` (`# What is searched`), `issues/remap.rs` (the duplicate scan) |
| A code point computed from a match (`U+[BASE+]($N)`), and why it is hexadecimal on both sides | `exists.rs` (`eval_codepoint`) |
| Where a scoped item is expanded, and why a `map` unrolls per match where a `glyph` block does not | `render/ttf_builder/expand.rs` (`expand_inner`), `issues/mod.rs` (`Cx::source_items`) |
| Why several groups combine by the largest (not the LCM), and what a ragged group warns | `pattern.rs`, `issues/patterns.rs` (`check_ragged_patterns`) |
| Stating one line for several slices (`map wide\|narrow :`) and per-slice `name-parts` | `document/name_parts.rs` (`SliceNameParts`), `pattern.rs` |
| `glyph A = B`: one glyph id, two names; where each stage canonicalizes | `alias.rs` |
| Several names of one pattern block turning out to be one glyph, and why two blocks never merge | `merge.rs` |
| Why a merge is decided on names and not on outlines, and what makes that sound | `merge.rs` (`implicit_merges`), `document/name_parts.rs` (`expand_glyph_block_slots`) |
| Which glyph a `remap` stops from merging, and why only the ones it matches on | `merge.rs` (`remap_inputs`) |
| Saying that each expansion of a pattern block is a glyph of its own | `document_io.rs` (`keep`), `merge.rs` |
| A `remap` rule that the lookup silently drops, and where that is reported | `render/ttf_builder/gsub.rs` (`shadowed_single_subst_rules`, `build_single_subst_from_pairs`) |
| Anchor exposure and bearings | `ref_composite/mod.rs` |
| `⿰⿱⿲⿳`: the split, the gap term, and why the offsets are derived rather than written | `compose.rs` |
| The `:WxH-l` variant name rule, and the position tie-break between same-sized variants | `compose.rs` (`VariantSpec`, `direction_rank`) |
| An IDC line written as a pattern, and why its layout is still solved per glyph | `compose.rs`, `document/name_parts.rs` (`expand_glyph_block`) |
| What a pattern glyph block shares with every name it declares (the grid, the box, the flags) | `document/name_parts.rs` (`expand_glyph_block`) |
| Clearance: the ink a split leaves between its parts and the box, and why the per-part range and the total are both needed | `compose.rs` (`InkProfile`, `measure_clearances`) |
| `audit ideal-clearance PREFIX* MIN MAX`: the prefix match, and which rule wins | `audit.rs` (`IdealClearances`) |
| `audit max-contact-run PREFIX* N`: how far two parts may run together, and why that is a clearance rather than a complaint of its own | `audit.rs` (`MaxContactRuns`), `compose.rs` (`contact_run`) |
| Why a contact needs no hardblank term, and why the two never both fire on one junction | `compose.rs` (`contact_run`, `InkLine::ink`) |
| Why a contact is measured between two contours and not two cells, and the covers a profile keeps for it | `compose.rs` (`contact_run`, `EdgeCover`), `detail.rs` (`DetailRegion::edge_coverage`) |
| What a `uniform fix` command may rewrite, and the two frontends that apply one | `fix/mod.rs` |
| Optimizing clearance: the variant search, the score, and why the gaps are arithmetic and not a search | `fix/clearance.rs` (`optimize_clearance`, `arrange`) |
| Which of several equally good layouts is chosen, and why the edges are minimized first | `fix/clearance.rs` (`Key`) |
| Optimizing an IDC line written as a pattern: the gaps its glyphs share, and why the count of warning glyphs comes before the score | `fix/clearance.rs` (`optimize_pattern_line`) |
| Why a component's `:label` is the family's to choose where its base is not, and why one label must serve every glyph | `fix/clearance.rs` (`slot_choices`) |
| Which parts the variant search knows about, pattern-declared blocks included | `fix/clearance.rs` (`Inventory::collect`, `block_names`) |
| A fix in the editor: one undo entry per file, and why nothing is written to disk | `app/fix.rs` |
| Why a rule about the source is `audit` and not `meta` | `audit.rs` |
| Which parts a clearance check can measure, and what it costs a source with no rule | `render/ttf_builder/expand.rs` (`ink_profiles`) |
| Why a clearance is measured over the declared box, and what ink escaping it costs | `compose.rs` (`InkProfile::of`) |
| What a sample cell paints a background over, and why only its width is the box's | `render/sample.rs` (`sample_background`) |
| What `demo.html` embeds instead of rendered output, and the four ways its specimen differs from the editor's | `render/demo/mod.rs` |
| Which character names the demo page is told and which it spells for itself | `render/demo/mod.rs` (`collect_names`), `render/demo/demo.js` (`nameOf`) |
| Why the demo's cells are rendered in lazy chunks, and why a chunk knows its height before its content | `render/demo/demo.js` |
| One face's bitmap and vector builds with no cache between them (the demo's pair, as against the editor's) | `render/ttf_builder/mod.rs` (`build_face_ttf_pair`) |
| Why an IDC line becomes `ref`s at expansion time, and why the parts are sized by what they *declare* | `render/ttf_builder/expand.rs` (`expand_compose_lines`), `ref_composite/mod.rs` (`declared_box`) |
| Why an anchor error drops the glyph (and so its cmap entry), like a missing ref | `render/glyph_cache.rs` (`resolve_pending`) |
| What each severity means, and which of them a build, `uniform test` and CI may ignore | `issues/mod.rs` (`Severity`) |
| Which glyph a finding is about, and why a composite carries its components' findings | `glyph_flags.rs` |
| When a finding faults one expansion of a pattern rather than the whole line, and the only two paths that can | `resolve.rs` (`Diagnostic::glyph`), `glyph_flags.rs` |
| The specimen's warning/error cell tints, and why the hovered cell inverts instead of hiding one | `specimen.rs` (`flag_bg`) |
| Why a click on a tinted cell lands on the component rather than on the character's own glyph | `specimen.rs` (`goto_target`), `glyph_flags.rs` (`GlyphFlags::source`) |
| Why an IDC line with an unpicked variant is a TODO and not an error, and what else it silences | `compose.rs` (`expand_compose`, `is_undecided`) |
| Why the ref an unpicked component derives is left unresolved *and* unreported | `render/ttf_builder/expand.rs` (`expand_compose_lines`) |
| The Issues tab's per-severity filter, why notes start hidden and why right-click is solo | `app/panels.rs` (`IssueFilter`) |
| The two rectangles a resize drags — the box (F2) and the canvas (under the backreference shadow) — and why only the box's drag moves a `ref` to the glyph | `editor/glyph_resize.rs`, `app/resize.rs` |
| Why growing the canvas writes an `origin`, and what else it pins | `editor/glyph_resize.rs` (`canvas_box`) |
| Why a canvas drag only switches modes once it has a pixel to show | `editor/glyph_resize.rs` (`CanvasStart`) |
| Which flags a box drag writes, and why a vertical one states the height | `editor/glyph_resize.rs` (`boxed_for`), `document_io.rs` (`replace_glyph_box_flags`) |
| Which `ref` a resize may rewrite: named outright, and not anchor-placed | `editor/glyph_resize.rs`, `ref_composite/anchors.rs` (`DeriveOutcome::anchor_placed`) |
| Inlining a `ref` one level (`Inline once`) vs. flattening it to pixels | `editor/document_view/changes.rs` (`inline_ref_once`), `ref_composite/mod.rs` (`InlineSource`) |
| On-demand glyph names, `BitmapFill`, circles and polygons | `on_demand.rs` |
| `glyph … desync`: a grid the bitmap face draws and the vector face ignores | `render/ttf_builder/mod.rs`, `ref_composite/mod.rs` (`ResolvedGlyph`) |
| Why the view synthesizes an on-demand ref instead of waiting for the resolve | `ref_composite/mod.rs` (`resolve_ref_name_for_view`) |
| Why a `ref` to a composite that subtracts is one sample layer, not its parts | `render/sample.rs` (`push_ref_components`) |
| Sub-pixel shape codes, `PX_CUSTOM` | `pixel.rs` |
| `$$`, the blank that is not `..`, and why it is an id rather than a spare bit combination | `pixel.rs` (`PX_HARDBLANK`) |
| A hardblank is a *claim*, not geometry: how claims and ink combine, and why only a claim cancels a claim | `pixel.rs` (`blank_op`) |
| Why a rescale carries a claim (and a bare ink flag) by hand, beside the geometry sweep | `document/pixel_grid.rs` (`PixelGrid::rescale`) |
| The three questions asked of a cell (bitmap ink / vector contour / nothing at all) and the `CLEAR`-`HARDBLANK`-`INK` ladder | `pixel.rs` (module docs), `compose.rs` (`InkProfile::of`) |
| Snapping an exact region back onto the catalog (a grid on its way into a file) | `detail.rs` (`nearest_shape`), `document/pixel_grid.rs` (`snap_details_to_catalog`) |
| Why the exact sweep carries no rational arithmetic, and the width budget that bounds it | `detail.rs` (`Frac`, `MAX_SWEEP_COORD`) |
| The shape palette: rotation orbits, and rotation as separate state | `editor/glyph_widget.rs` |
| Feature targets, `DFLT`/LangSys fallback | `render/ttf_builder/gsub.rs` |
| A remap group is one lookup: rule order is match priority | `render/ttf_builder/gsub.rs` |
| Lookup order, `remap group` and its stable toposort | `document/remap.rs` (`remap_group_order`) |
| `assert shape` and why `@lang` is BCP 47 | `render/assert.rs` |
| What a test run builds (lazily, once per face), and how the editor's stays fast | `render/assert.rs` (`run_assertions_inner`), `app/background.rs` (`run_shape_assertions`) |
| Contour coordinate spaces | `render/contour.rs` |
| The editor as a widget; what is per-instance vs per-pane | `editor/mod.rs`, `editor/ids.rs` |
| Folding a glyph block: what a group is, and when the group list is recomputed | `editor/folding.rs` |
| `#`/`##`/`###` headings: the syntax, and why they are a comment to every build stage | `document_io.rs` (`# Headings`), `document/mod.rs` (`DocumentItem::Heading`) |
| What a heading section holds, and why one lone `#` folds nothing | `editor/folding.rs` (`fold_groups`) |
| Why a heading draws in zoom *steps*, and what marks the minimap | `document_view/layout.rs` (`heading_font_size`), `editor/minimap.rs` |
| Where a caret goes when a fold swallows the line it was on, and what a fold does to a selection | `editor/folding.rs` (`toggle_at`, `snap_caret`) |
| Which glyph blocks start folded, and the font height that decides it | `editor/folding.rs` (`apply_initial`) |
| Why closing a fold may scroll and opening one may not | `editor/folding.rs` (`FoldScroll`), `document_view/scroll.rs` |
| The gutter's marker columns: one per nesting level, outermost by the numbers, and why the count is the document's rather than the page's | `document_view/layout.rs` (`GutterLayout`, `page_has_fold_marker`), `editor/folding.rs` (`nesting_depth`) |
| Why wrapping is measured against the widest gutter, not the reserved one | `document_view/mod.rs` (`wrap_width`) |
| Split panes, their invariants and key chords | `app/panes.rs` |
| Go back / go forward | `app/history.rs` |
| The Search pane, and where a click goes with no definition | `app/search.rs` |
| Why a search lists a name written as a pattern, and what makes that cheap | `app/search.rs` (`pattern_denotes`, `may_write_a_pattern`), `pattern.rs` (`NamePattern::matches`) |
| Why a Ctrl/Cmd+click reads no files at all | `app/docs.rs` (`FontSource`), `app/search.rs` |
| Why opening a file from the snapshot keeps its generations (and so rebuilds nothing) | `app/docs.rs` (`open_document_from_text`) |
| Typing a character by code point (Ctrl+K), and why not Alt | `editor/codepoint_popup.rs` |
| Why a click on a caret-anchored popup's own chrome does not dismiss it, and what its commit button does instead | `editor/codepoint_popup.rs` (`resolve_field`) |
| Why completing a glyph name stops filtering at its last `:`, and the order an IDC slot puts that listing in | `editor/autocomplete.rs` (`effective_prefix`, `filter_candidates`), `compose.rs` (`direction_rank`) |
| Why an IDC slot's listing drops a variant of the wrong size outright, where a wrong direction is only ordered last | `editor/autocomplete.rs` (`CrossExtent`) |
| The `{gc=… ccc=… eaw=…}` group after a character name, and the pinned UCD version | `ucd.rs` |
| `prop`: naming Private Use characters the UCD says nothing about, and what reads it | `ucd.rs` (`CharProps`) |
| Which block a code point is in, and how `prop block` overrides the UCD | `ucd.rs` (`BlockMap`) |
| Which Private Use characters exist (`prop` replaces the UCD there), and the block coverage counting them | `ucd.rs` (`CharProps::is_assigned`), `specimen.rs` |
| The specimen's three options, filling a block, and hiding an excluded row | `specimen.rs` (`SpecimenOptions`) |
| Where a variation sequence sits on the specimen, the `+VS17` label it carries, and the undrawn border joining it to its base | `specimen.rs` (`UvsEntry`, `uvs_label`, `uvs_boundary`) |
| Why the specimen resolves the `exists` searches itself, and the one clone per line that pays for it | `specimen.rs` (`rebuild_if_needed`), `exists.rs` (`Scope::rebind`) |
| The text-editing keys, and the state both the editor and the preview edit through | `editor/doc_input.rs` (`TextEdit`) |
| How tall a preview row is, and why its chrome is measured from the face rather than the font size | `preview/metrics.rs` (`VMetrics`) |
| Why the editor's preedit box cannot crop a glyph but the preview's could | `editor/document_view/paint.rs`, `preview/metrics.rs` |
| A header and its grid are one block: Enter, line-wise copy/cut, paste onto it | `editor/editing.rs` (`insert_newline`), `editor/doc_input.rs` (`current_line_range`, `paste_text`) |
| Why an edit on a header or `ref` line waits before it reparses | `editor/document_view/changes.rs` (`apply_pending_rederive`) |
| Why a line the grammar cannot read does not fail the derive | `document_io.rs` (`derive_document`) |
| Why a menu action has to hand the keyboard back to the editor | `editor/mod.rs` (`refocus`), `editor/document_view/paint.rs` (`refocus_after_menu`) |
| A resize preview: uncommitted text, and everything that has to drop it | `editor/glyph_resize.rs` (`cancel`), `app/docs.rs` (`flush_pending_changes`) |
| A floating pixel selection: what commits it, and who lands it before reading the buffer | `editor/pixel_selection.rs` (`reconcile`), `app/docs.rs` (`commit_floating_selection`) |
| Copy/Cut/Delete with nothing framed, and the corner a shift-click extends from | `editor/pixel_selection.rs` (`effective_selection`, `select_all`), `editor/mod.rs` (`pixel_select_anchor`) |
| The empty band below the last line: why the canvas fills the viewport, and where a click there lands | `editor/document_view/paint.rs` (`paint_document_area`) |
| Who owns a key while an IME is composing (Korean vs Japanese) | `editor/doc_input.rs` (`ImeKeyGuard`) |
| Which tokens on a line name what | `editor/line_fields.rs` |
| Inline annotations: one caret step, but ordinary text to the line breaker | `editor/annotations.rs`, `editor/visual_lines.rs` (`compute_wrap_segments`) |
| The metrics overlay | `editor/grid_render.rs`, `editor/document_view/layout.rs` |
| The anchor shadow | `editor/anchor_shadow.rs` |
| The backreference shadow, and why it is a toggle inside pixel selection rather than always on | `editor/backref_shadow.rs`, `editor/mod.rs` (`EditMode::PixelSelect`) |
| What the two shadows share: the union rule, the placement bound, the one live shadow | `editor/shadow.rs` |
| Files changed outside the editor: reload, keep-and-warn, overwrite guards | `app/watch.rs` |
| Why a polled directory is enumerated rather than `stat`ed, and how its interval sets itself | `app/watch.rs` (`poll_snapshot`, `next_poll_delay`) |
| Rebuild debouncing, generations and cache keying | `app/background.rs`, `specimen.rs` |
| Where the seconds before the first frame go (and what `before main()` does and does not prove) | `startup.rs` |
| Why startup and Open Folder build no font of their own | `app/background.rs` (`arm_initial_font_build`) |
| Why the directory load reads its files on many threads | `render/ttf_builder/mod.rs` (`load_docs_from_directory_with_sources`) |
| One build at a time, and cancelling the one that a new edit superseded | `app/background.rs`, `cancel.rs` |
| Why a resolution round is a *wave*, and what a wave member may not depend on | `render/glyph_cache.rs` (`resolve_pending`), `ref_composite/mod.rs` (`resolve_expansion_cached`) |
| Splitting a memo off its tracer so the tracer can leave the thread | `render/glyph_cache.rs` (`CompositeBuilder`), `render/ttf_builder/contours.rs` (`ContourBuilder`) |
| Which build stages run at once, and what they must not share to | `render/ttf_builder/mod.rs` (`build_faces`, `build_font_pair_cached_for`), `contours.rs` (`ContourCaches`) |
| Why only the union face is traced, and what a secondary face costs instead | `render/ttf_builder/mod.rs` (`build_faces`), `collect.rs` (`collect_face_cmap`), `expand.rs` (`expand_maps_for`) |
| Which of a `build`'s outputs are produced at once, and the one that has to wait | `main.rs` (`OutputWork`), `render/sample.rs` (`SampleSource`) |
| Who shares the primary face's expansion, and why it is computed beside the build | `main.rs` (the `build` thread scope), `resolve.rs` (`Resolution`), `render/sample.rs` (`collect_sample_data_with`) |
| Dropping a composite that can never resolve before the expensive loop sees it | `render/glyph_cache.rs` (`drop_unresolvable`) |
| Why a resolve recomposes only what an edit reached (and why it used to trail the build) | `ref_composite/mod.rs` (`CompositeGridCache`) |
| Which face the editor builds, and switching it | `app/background.rs` (`set_selected_face`) |
| Why the remembered face is applied before the first build, not after the first resolve | `app/mod.rs` (`with_settings`) |
| What survives between runs, what egui persists on its own, and why there is no session restore | `app/settings.rs` |
| Where the settings file lives, and the app id that decides it | `app/settings.rs`, `main.rs` (`with_app_id`) |

## Testing

- `cargo test` — ~1500 unit tests. Heaviest suites: `document_io_tests/` (parser round-trips),
  `editor/view_tests/` (GUI scenarios), `render/ttf_tests/`, `editor/doc_links.rs`, `pattern.rs`.
- **GUI behavior must be tested through `EditorHarness` (`src/editor/harness.rs`)**, not left to
  manual testing; scenarios go in `src/editor/view_tests/`, one module per theme. The harness docs say what it drives and
  what it papers over.
- Golden snapshots (`src/golden.rs`) cover the diagnostics report and a digest of what resolution
  produces over `testdata/`. Behaviour-preserving refactors must not move them; intentional changes
  update them so the diff is reviewable. Regenerate with `UNIFORM_UPDATE_GOLDEN=1 cargo test golden`.
- `assert` directives in `font/*.unf` are the font-level regression suite; run with `make test`.
  Prefer adding an `assert same/distinct` or `assert shape` when fixing a glyph-level bug.
- Per project policy: write the regression test first, observe the failure, then fix.

### Where the tests live

Small `#[cfg(test)] mod tests` blocks stay at the bottom of the module they test. Where the suite grew
past the source it tests, it lives in a sibling file (or directory) declared as a *child* module through
`#[path]`, so it still reaches the module's private items:

| Module | Tests |
| --- | --- |
| `render/ttf_builder/` | `render/ttf_tests/` — `misc`, `hints`, `gsub`, `gpos`, `color`, `composite`, `collection`, with shared canonicalization helpers in its `mod.rs` |
| `document_io.rs` | `document_io_tests/` — `roundtrip`, `doclines`, `derive`, `lenient`, `tokenizer`, `maps`, `colors`, `asserts`, `comments`, `misc`, `at_names` |
| `document/` | `document/document_tests.rs` |
| `issues/` | `issues/issues_tests.rs` |
| `exists.rs` | `exists_tests.rs` |
| `pixel.rs` | `pixel_tests.rs` |
| `specimen.rs` | `specimen_tests.rs` |
| `render/sample.rs` | `render/sample_tests.rs` |
| `editor/pixel_selection.rs` | `editor/pixel_selection_tests.rs` |
| `editor/glyph_resize.rs` | `editor/glyph_resize_tests.rs` |
| `meta.rs` | `meta_tests.rs` |
| `faces.rs` | `faces_tests.rs` |
| `ref_composite/` | `ref_composite/ref_composite_tests.rs` |
| `on_demand.rs` | `on_demand_tests.rs` |
| `compose.rs` | `compose_tests.rs` |
| `fix/clearance.rs` | `fix/clearance_tests.rs` |
| `editor/document_view/` | `document_view/tests.rs` (helpers) and `editor/view_tests/` (harness scenarios, with the shared fixtures in its `mod.rs`) |

Keep a source file at roughly 2000 lines or under; split by stage (as `ttf_builder/` and
`document_view/` are) rather than growing one file further. A test suite that outgrows its sibling
file becomes a directory of its own, grouped by what it tests (`document_io_tests/`,
`editor/view_tests/`).

### `font/` is a consumer, not a part of Uniform

**Never let an automated test read `font/`.** It is downstream data that happens to live in the same
repo; it changes for font-design reasons, so a test bound to it fails for reasons unrelated to the
code under test, and it is far too large to be a meaningful fixture.

When `font/` turns up a bug, **extract or inline the case**: add a minimal `.unf` to `testdata/`, or
build the grid/document inline in the test. Reproduce the shape of the problem, not the real glyph.
Ad-hoc *manual* runs against `font/` (`cargo run -r -- build -i font/`, `make test`) are fine and
expected — the prohibition is on `cargo test` depending on it. The one in-tree exception is the
`#[ignore]`d manual profiling harness `ref_composite::tests::profile_resolve_name_expansion`, which
needs realistic scale and never runs in a default `cargo test`; keep any such case `#[ignore]`d.

## Where the bugs come from

Ranked by how often recent commits touched them for a *fix* rather than a feature. Each module's own
docs carry the detail; this is the ranking.

1. **`render/ttf_builder/` + `render/contour.rs`** — by far the most churn. Contour tracing over
   sub-pixel and on-demand shapes, seen through a composite rather than alone.
2. **`detail.rs` / `pixel.rs` geometry** — degenerate cases when merging two shapes, extremely tiny
   glyphs, zero-width pixel grids. Exact-rational sweeps make these correct-by-construction only if
   the degenerate inputs are actually handled; test empty/1×1/zero-extent inputs explicitly.
3. **`render/sample.rs` + `specimen.rs`** — usually "the font is right, the sample is wrong". When
   fixing a rendering bug, check both the TTF and the sample.
4. **`editor/document_view/` and the interaction layer** — focus capture, wheel scroll over the
   pixel grid, delete-key behavior, lost glyph flags after dragging a layer. These are exactly the
   regressions `EditorHarness` exists for.
5. **Name expansion and remap** (`pattern.rs`, `ttf_builder/gsub.rs`) — empty remap targets,
   missing remap warnings, Hangul composition rules. The context-dependent parse rules are the trap.
6. **Performance regressions count as bugs here** — a slow `resolve` used to snowball into dozens of
   concurrent rebuild threads. The build's expensive stages now run on every core (`parallel.rs`),
   which makes *shared mutable state added to one of them* the new version of that bug: a memo a
   stage carries has to sit on the serial side of the split, the way `ContourBuilder` holds its
   `ContourCache`. `UNIFORM_PERF`, the rebuild guard in `app/background.rs`, memoized exact
   subtraction and the `PixelGrid::rescale` caches all exist because of that. Keep the caches keyed
   correctly when changing geometry. The same rule covers the filesystem: this editor is routinely
   run against a network share, where a per-file round trip costs ~185 ms, so **nothing on the UI
   thread may read a directory file by file or build the font** — `startup.rs` is how that is
   measured and `arm_initial_font_build` is where the work goes instead.
