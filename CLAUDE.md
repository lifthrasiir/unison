# Uniform

Bitmap font editor with sub-pixel shape support and a TTF builder, plus the `font/` sources of the
Unison font itself. egui/eframe GUI, Rust 2024 edition. Single binary `uniform` with three modes:
GUI (default), `build`, and `test`.

**This file is an index.** The reasoning behind each design — the `.unf` format, composition rules,
the editor's structure — lives in the module-level `//!` docs of the code that
implements it; this file says which module that is. Keep it that way: when a new invariant is worth
recording, put it next to the code and add at most a line here.

## Build & Run

```sh
cargo build -r    # normal build
cargo test        # unit + golden + GUI-harness tests
make              # build unison.ttf/.woff2 + sample.html/sample.png/live.html
make test         # the above, then run the `assert` directives in font/
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

The `build`/`test` subcommands require native execution:

```sh
cargo run -r -- build -i font/ -o unison.ttf [-o unison.woff2] [-o unison-%.ttf] [-o unison.ttc] \
    [--sample-html F] [--sample-png F] [--live-html F] [-d data]
cargo run -r -- test -i font/       # run `assert` directives; exit 1 on failure
```

Output extension picks the format (`.woff2` → WOFF2, anything else → TTF). Both subcommands print
parse errors per file and then the full `issues.rs` validation report (`error:`/`warning:` with
`file:line:`); the font still builds when only warnings/refs-to-nothing exist, so read the report.

The GUI takes an optional font-directory argument: `cargo run -r -- font/`.

### Environment variables

| Var | Effect |
| --- | --- |
| `UNIFORM_PERF` | `[perf]` per-stage timing logs for font/derived-data rebuilds (`app/background.rs`) |
| `UNIFORM_UPDATE_GOLDEN=1` | Rewrite `testdata/*.golden` instead of comparing (`cargo test golden`) |
| `UNIFORM_WATCH_POLL_MS` | Re-scan interval used when the font directory is on a network volume (default 10000; `app/watch.rs`) |
| `UNIFORM_PROFILE_RUNS` | Iteration count for the `ref_composite` profiling test |

Cargo features: `editor` (default) pulls in eframe/egui/tiny-skia/notify/rfd/arboard. `--no-default-features`
builds the headless CLI only — **it breaks easily**, since most code is under `#[cfg(feature = "editor")]`;
check it when touching module boundaries (there is a past commit fixing exactly this rot).

## Source Layout

Core (feature-independent):

- `document.rs` / `document_io.rs` — the `.unf` data model, parser and serializer. **`document_io.rs`
  is the format reference**: tokens, comments, every directive, glyph blocks and their flags.
- `alias.rs` — `glyph NAME = TARGET`: a second *name* for a glyph, sharing its glyph id. Holds the
  chain/cycle rules and the list of which pipeline stages canonicalize where.
- `pattern.rs` — `NamePattern`, the single name-expansion engine. The same syntax parses differently
  per context on purpose; its module docs spell the three contexts out.
- `pixel.rs` — `PixelShape`/`PixelGrid`, the shape-code catalog (`PX_*`), boolean ops, `rescale`.
- `detail.rs` — `DetailRegion`: exact per-pixel sub-pixel geometry on a `1/den` lattice, combined by
  an exact trapezoid sweep. This is what makes composition exact instead of code-approximate. The
  sweep's arithmetic is bounded by construction (`MAX_SWEEP_COORD` carries the width budget); read
  `Frac` before adding arithmetic to it.
- `ref_composite.rs` — composite (`ref`) resolution. Its module docs hold two things nothing else
  records: **anchor exposure is opt-in** and a **negative `ref` offset is a bearing**.
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
- `meta.rs` — the `meta` directive: the key set, the `@LANG` language slot, and which font fields are
  *declared*, *derived* and *computed*. Tests in `meta_tests.rs`. Values on the pixel grid are
  declared in pixels and scaled by the builder, like everything else in `.unf`.
- `math.rs` — the one gcd/lcm of the geometry code (binary GCD). A helper more than one module
  wants lives here rather than being re-derived per module.
- `cancel.rs` — `CancelToken`: how a background stage is told its result is no longer wanted, and
  what a cancelled stage is allowed to return. Only the editor cancels; every other caller passes
  `CancelToken::never()`.
- `issues.rs` — cross-document validation (missing refs, duplicate maps, unused glyphs, remap sanity).
- `script_run.rs` — script segmentation for shaping, mirroring browser behavior.
- `ucd.rs` — the character properties shown beside a character name, and the `prop` directives a
  source states them with (`CharProps`). Nothing in the font depends on them; the status bars and the
  `sample.html` tooltips do. Also `BlockMap`: the bundled `Blocks.txt` with the source's own
  `prop block` claims over it.
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
  split-editor model and its two invariants), `rename.rs`, `search.rs` (the Search pane),
  `settings.rs` (what survives between runs, and what egui persists instead),
  `watch.rs` (the OS watch on the font directory and what an external change may do), `toast.rs`,
  `zoom.rs`.
- `editor/mod.rs` — `EditorState`, `EditMode`, and **the editor-is-a-widget model**; `editor/ids.rs`
  is the per-instance `egui` id namespace it rests on.
- `editor/document_view/` — the editor widget: `DocumentEditor::show` and the `show_document` frame
  loop behind it (most churn in the editor). `mod.rs` is the loop and the view cache; `layout.rs`
  (grid extents/strips, the visual-line model, `GlyphMetrics`), `paint.rs`, `scroll.rs`, `keys.rs`,
  `popups.rs`, `changes.rs`.
- `editor/` others — `anchor_shadow`, `caret`, `codepoint_popup`, `visual_lines`, `line_fields` (**the single place that
  knows where names live** on a line), `doc_links`, `doc_input`, `editing`, `reconcile`, `undo`,
  `autocomplete`, `annotations`, `colors`, `minimap`, `inline_tools`, `glyph_widget`, `grid_render`
  (grid painting and the metrics overlay), `pixel_interaction`, `pixel_selection`, `harness`,
  `view_tests`.
- `sidebar.rs` — `.unf` file list (open, rename, create). `specimen.rs` — specimen rendering; its
  three cache keys (documents, `SpecimenOptions`, column count) are documented there and are easy to
  get wrong.
- `edit_menu.rs`, `preview/` — bottom live-preview panel: rustybuzz shaping + platform rasterizer
  (`coretext.rs` on macOS, `directwrite.rs` on Windows). `preview/widget.rs` is a multi-line text
  field that runs on the *editor's* text model and key handler; only its layout is its own.

`font/*.unf` are the font sources (one file per category). `testdata/` holds test-only `.unf` files
plus goldens. `data/` holds sample-generation inputs (confusables, UDHR text) read at build time
through `-d data`, plus `Blocks-17.0.0.txt`, which is the one file there compiled *into* the binary
(`include_str!` from `ucd.rs`) because the editor needs it with no `-d` in sight.

### Where a given design is written down

| Topic | Read |
| --- | --- |
| `.unf` syntax: tokens, comments, directives, glyph blocks | `document_io.rs` |
| What characters a name may contain | `document_io.rs` (`# Names`), `pattern.rs` |
| `@` as a glyph/`ref` name prefix: what it stands for and where the written form is kept | `document.rs` (`expand_at_name`), `document_io.rs` (`# Names`) |
| `meta` keys, name-record derivation, single-assignment rule | `meta.rs` |
| Faces, slices, the base slice, and why there is no override | `faces.rs` |
| `--output` path rules (`%`, `.ttc`, `.woff2`) | `faces.rs` (`plan_output`) |
| Writing a TTC, and what the faces of one share | `render/ttf_builder/collection.rs` |
| Why the glyph order is face-independent | `render/ttf_builder/collect.rs`, `build_faces` in `mod.rs` |
| Why a glyph's GID is its index, and how `.notdef` gets to GID 0 | `render/ttf_builder/mod.rs` (`NOTDEF`), `collect.rs` |
| `ulUnicodeRange`/`ulCodePageRange` derivation from the cmap | `render/ttf_builder/os2_ranges.rs` |
| Name pattern grammar and its per-context parses | `pattern.rs` |
| Stating one line for several slices (`map wide\|narrow :`) and per-slice `name-parts` | `document.rs` (`SliceNameParts`), `pattern.rs` |
| `glyph A = B`: one glyph id, two names; where each stage canonicalizes | `alias.rs` |
| Anchor exposure and bearings | `ref_composite.rs` |
| On-demand glyph names, `BitmapFill`, circles and polygons | `on_demand.rs` |
| `glyph … refonly`: a grid the bitmap face draws and the vector face ignores | `render/ttf_builder/mod.rs`, `ref_composite.rs` (`ResolvedGlyph`) |
| Why the view synthesizes an on-demand ref instead of waiting for the resolve | `ref_composite.rs` (`resolve_ref_name_for_view`) |
| Why a `ref` to a composite that subtracts is one sample layer, not its parts | `render/sample.rs` (`push_ref_components`) |
| Sub-pixel shape codes, `PX_CUSTOM` | `pixel.rs` |
| Snapping an exact region back onto the catalog (a grid on its way into a file) | `detail.rs` (`nearest_shape`), `document.rs` (`snap_details_to_catalog`) |
| Why the exact sweep carries no rational arithmetic, and the width budget that bounds it | `detail.rs` (`Frac`, `MAX_SWEEP_COORD`) |
| The shape palette: rotation orbits, and rotation as separate state | `editor/glyph_widget.rs` |
| Feature targets, `DFLT`/LangSys fallback | `render/ttf_builder/gsub.rs` |
| A remap group is one lookup: rule order is match priority | `render/ttf_builder/gsub.rs` |
| Lookup order, `remap group` and its stable toposort | `document.rs` (`remap_group_order`) |
| `assert shape` and why `@lang` is BCP 47 | `render/assert.rs` |
| What a test run builds (lazily, once per face), and how the editor's stays fast | `render/assert.rs` (`run_assertions_inner`), `app/background.rs` (`run_shape_assertions`) |
| Contour coordinate spaces | `render/contour.rs` |
| The editor as a widget; what is per-instance vs per-pane | `editor/mod.rs`, `editor/ids.rs` |
| Split panes, their invariants and key chords | `app/panes.rs` |
| Go back / go forward | `app/history.rs` |
| The Search pane, and where a click goes with no definition | `app/search.rs` |
| Why a Ctrl/Cmd+click reads no files at all | `app/docs.rs` (`FontSource`), `app/search.rs` |
| Typing a character by code point (Ctrl+K), and why not Alt | `editor/codepoint_popup.rs` |
| The `{gc=… ccc=… eaw=…}` group after a character name, and the pinned UCD version | `ucd.rs` |
| `prop`: naming Private Use characters the UCD says nothing about, and what reads it | `ucd.rs` (`CharProps`) |
| Which block a code point is in, and how `prop block` overrides the UCD | `ucd.rs` (`BlockMap`) |
| Which Private Use characters exist (`prop` replaces the UCD there), and the block coverage counting them | `ucd.rs` (`CharProps::is_assigned`), `specimen.rs` |
| The specimen's three options, filling a block, and hiding an excluded row | `specimen.rs` (`SpecimenOptions`) |
| The text-editing keys, and the state both the editor and the preview edit through | `editor/doc_input.rs` (`TextEdit`) |
| A header and its grid are one block: Enter, line-wise copy/cut, paste onto it | `editor/editing.rs` (`insert_newline`), `editor/doc_input.rs` (`current_line_range`, `paste_text`) |
| Why an edit on a header or `ref` line waits before it reparses | `editor/document_view/changes.rs` (`apply_pending_rederive`) |
| Why a line the grammar cannot read does not fail the derive | `document_io.rs` (`derive_document`) |
| Why a menu action has to hand the keyboard back to the editor | `editor/mod.rs` (`refocus`), `editor/document_view/paint.rs` (`refocus_after_menu`) |
| A floating pixel selection: what commits it, and who lands it before reading the buffer | `editor/pixel_selection.rs` (`reconcile`), `app/docs.rs` (`commit_floating_selection`) |
| Copy/Cut/Delete with nothing framed, and the corner a shift-click extends from | `editor/pixel_selection.rs` (`effective_selection`, `select_all`), `editor/mod.rs` (`pixel_select_anchor`) |
| Who owns a key while an IME is composing (Korean vs Japanese) | `editor/doc_input.rs` (`ImeKeyGuard`) |
| Which tokens on a line name what | `editor/line_fields.rs` |
| The metrics overlay | `editor/grid_render.rs`, `editor/document_view/layout.rs` |
| The anchor shadow | `editor/anchor_shadow.rs` |
| Files changed outside the editor: reload, keep-and-warn, overwrite guards | `app/watch.rs` |
| Rebuild debouncing, generations and cache keying | `app/background.rs`, `specimen.rs` |
| One build at a time, and cancelling the one that a new edit superseded | `app/background.rs`, `cancel.rs` |
| Which face the editor builds, and switching it | `app/background.rs` (`set_selected_face`) |
| What survives between runs, what egui persists on its own, and why there is no session restore | `app/settings.rs` |
| Where the settings file lives, and the app id that decides it | `app/settings.rs`, `main.rs` (`with_app_id`) |

## Testing

- `cargo test` — ~680 unit tests. Heaviest suites: `document_io_tests.rs` (parser round-trips),
  `editor/view_tests.rs` (GUI scenarios), `render/ttf_tests/`, `editor/doc_links.rs`, `pattern.rs`.
- **GUI behavior must be tested through `EditorHarness` (`src/editor/harness.rs`)**, not left to
  manual testing; scenarios go in `src/editor/view_tests.rs`. The harness docs say what it drives and
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
| `document_io.rs` | `document_io_tests.rs` |
| `meta.rs` | `meta_tests.rs` |
| `faces.rs` | `faces_tests.rs` |
| `ref_composite.rs` | `ref_composite_tests.rs` |
| `on_demand.rs` | `on_demand_tests.rs` |
| `editor/document_view/` | `document_view/tests.rs` (helpers) and `editor/view_tests.rs` (harness scenarios) |

Keep a source file at roughly 2000 lines or under; split by stage (as `ttf_builder/` and
`document_view/` are) rather than growing one file further.

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
   concurrent rebuild threads. `UNIFORM_PERF`, the rebuild guard in `app/background.rs`, memoized exact
   subtraction and the `PixelGrid::rescale` caches all exist because of that. Keep the caches keyed
   correctly when changing geometry.
