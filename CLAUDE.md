# Uniform

Bitmap font editor with sub-pixel shape support and a TTF builder, plus the `font/` sources of the
Unison font itself. egui/eframe GUI, Rust 2024 edition. Single binary `uniform` with three modes:
GUI (default), `build`, and `test`.

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

The `build`/`test` subcommands require native execution:

```sh
cargo run -r -- build -i font/ -o unison.ttf [-o unison.woff2] \
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
| `UNIFORM_PERF` | `[perf]` per-stage timing logs for font/derived-data rebuilds (`app.rs`) |
| `UNIFORM_UPDATE_GOLDEN=1` | Rewrite `testdata/*.golden` instead of comparing (`cargo test golden`) |
| `UNIFORM_STACKMON=1` | Enable the stack-overflow monitor (see below) |
| `UNIFORM_STACKMON_DUMP_PCT` | Backtrace threshold as % of stack, default 40 |
| `UNIFORM_STACKMON_QUIET` | Suppress the routine stackmon chatter |
| `UNIFORM_STACKMON_LOG` | Log path, default `./uniform-stackmon.log` |
| `UNIFORM_PROFILE_RUNS` | Iteration count for the `ref_composite` profiling test |

Cargo features: `editor` (default) pulls in eframe/egui/tiny-skia/notify/rfd/arboard. `--no-default-features`
builds the headless CLI only — **it breaks easily**, since most code is under `#[cfg(feature = "editor")]`;
check it when touching module boundaries (there is a past commit fixing exactly this rot).

## Source Layout

Core (feature-independent):

- `document.rs` / `document_io.rs` — the `.unf` data model, parser and serializer. `DocLine` is the
  line-level model the editor edits; parsing is incremental (pixel-only edits do not reparse the file).
- `pattern.rs` — `NamePattern`, the single name-expansion engine (see *Name patterns* below).
- `pixel.rs` — `PixelShape`/`PixelGrid`, the shape-code catalog (`PX_*`), boolean ops, `rescale`.
- `detail.rs` — `DetailRegion`: exact per-pixel sub-pixel geometry on a `1/den` lattice, combined by
  an exact-rational trapezoid sweep. This is what makes composition exact instead of code-approximate.
- `ref_composite.rs` — composite (`ref`) resolution, anchor/point alignment, on-demand glyph synthesis.
- `resolve.rs` — shared vocabulary for the resolution pipeline (`ItemRef` provenance, `Diagnostic`),
  so build/editor/validation cannot drift apart. Resolution emits issues directly.
- `issues.rs` — cross-document validation (missing refs, duplicate maps, unused glyphs, remap sanity).
- `script_run.rs` — script segmentation for shaping, mirroring browser behavior.
- `render/` — `contour.rs` (pixel shapes → contours), `ttf_builder.rs` (contours → TrueType, GSUB,
  cmap; `UNITS_PER_EM = 1024`), `glyph_cache.rs` (shared composite-resolution driver used by both
  `ttf_builder` and `sample`), `sample.rs` (sample HTML/PNG/live HTML), `assert.rs` (`assert` directives).
- `stackmon.rs` — stack-overflow watchdog (below).
- `golden.rs` — `cfg(test)` golden snapshots over `testdata/`.

Editor (feature `editor`):

- `app.rs` — `UniformApp` eframe entry point; open documents, background font build, derived data
  (resolved glyphs, issues). Rebuilds are debounced (300 ms, 1000 ms after text input) and guarded
  against overlapping rebuild threads.
- `editor/mod.rs` — `EditorState`, `EditMode` (`Normal` text editing, `GlyphEdit` pixel painting,
  `LayerMove` ref/layer repositioning).
- `editor/document_view.rs` — the `show_document` frame loop (largest editor file, most churn).
- `editor/` others — `caret`, `visual_lines`, `line_fields`, `doc_links`, `doc_input`, `editing`,
  `reconcile`, `undo`, `autocomplete`, `annotations`, `colors`, `minimap`, `inline_tools`,
  `glyph_widget`, `grid_render`, `pixel_interaction`, `pixel_selection`, `harness`, `view_tests`.
- `sidebar.rs` — `.unf` file list (open, rename, create). `specimen.rs` — specimen rendering.
- `edit_menu.rs`, `preview/` — bottom live-preview panel: rustybuzz shaping + platform rasterizer
  (`coretext.rs` on macOS, `directwrite.rs` on Windows).

`font/*.unf` are the font sources (one file per category). `testdata/` holds test-only `.unf` files
plus goldens. `data/` holds sample-generation inputs (confusables, UDHR text).

### `line_fields.rs` — where names live

`LineField`/`FieldRole` is the *single* place that knows which tokens on a line name an entity
(`GlyphDef`, `GlyphRef`, `NamePartsDef/Value`, `PointDef`, `ColorDef/Ref`, `RemapGroupRef`). Clickable
links, rename detection, rename mutation and completion of existing tokens all consume it. **Adding a
new directive form means describing it once here**, not in four features. (What completion offers
*between* tokens is a separate concern and stays in `autocomplete.rs`.)

### Name patterns (`pattern.rs`)

Consolidated from two historical engines in `document.rs`; `document.rs` re-exports the API for legacy
import paths. The same syntax parses differently by context, **on purpose**, backed by tests in `pattern.rs`:

- `NamePattern::parse_element` — map/remap operands, runtime refs. Top-level `|`/`*` without parens
  treats the whole string as one group, because `substitute_name_parts` splices `$var` as `v1|v2**N`
  with no parens. `a*2|b` → `a,a,b`.
- `NamePattern::parse` — glyph block names. Top-level `|` is a verbatim trimmed list: `a*2|b` → `"a*2","b"`.
- `NamePattern::parse_segments` — ref targets inside pattern glyph blocks: groups only.

`get(i)` is cyclic (`i % group len`); GSUB and `expand_glyph_block` combine patterns via `combined_len`
(LCM) and index in lock-step without materializing. Syntax: `(a|b*2|c*4)` alternatives with per-item
repeats, `(...**N)` whole-group multiplier, `($name-parts)` and `($name-parts*N)`, inline numeric/hex
ranges `$0..9` / `$1..4**10` (zero-padded to the start token's width), and `U+XXXX..YYYY` codepoint
ranges. Expansion is capped by `MAX_EXPANSION`.

All alternative forms mix freely inside one group: `(foo|$bar|baz*5|$#00..ff*3**2)`. A `*N` on a
`$name-parts`/inline-range reference distributes over *every* substituted value (`($foo*2|bar)` is
`(a*2|b*2|c*2|bar)`), which is what `substitute_name_parts` emits. `**N` is the whole-group
multiplier and is only meaningful at the group's end — anywhere else it is a syntax error, so
`(a|b**3|c)` is rejected and must be written `(a|b*3|c)`. `($foo**N)` at a group end keeps its
historical whole-group meaning.

## Document Format (.unf)

Parsed/serialized in `document_io.rs`. Tokens use backtick-quoting: `` `foo bar` `` for tokens with
spaces; four backticks (two to escape, two to quote) for a literal backtick. `//` starts a comment
(accepted on `assert` and most directives).

### Directives

- `font-meta height H ascent A descent D`
- `map CHAR = GLYPH` — cmap mapping. Decomposable codepoints are validated and can synthesize a glyph.
- `name-parts $NAME = token1 token2 ...`
- `color NAME = #RRGGBB[AA] [coloronly|monoonly]` — named palette entry.
- `remap FEATURE : [LOOKBEHIND... :] SOURCE... -> TARGET... [: LOOKAHEAD...]` — GSUB substitution.
  Source and target are *lists* of glyph names in all cases; an empty target means removal.
  The list lengths pick the lookup type: 1→1 single, 1→N (incl. 1→0) multiple, N→1 ligature.
  N→M and N→0 have no OpenType lookup type and are an **error** — `issues.rs` reports them rather
  than letting the builder emit something close-but-wrong.
- `feature NAME for SCRIPT... : REMAP_GROUP` — OpenType feature, script-filtered. Directives sharing
  a tag *and* a script are merged into one feature record (lookups accumulate in declaration order),
  because a shaper only ever finds the first record for a tag. Same tag under different scripts
  stays separate.
- `feature NAME for SCRIPT... : anchor ANCHOR_NAME` — anchor-driven (mark attachment) variant.
- `assert shape TEXT [+feat|-feat...] : GLYPH [advance N] [offset X Y] : GLYPH ...` — shaping assertion.
- `assert same NAME...` / `assert distinct NAME...` — resolved-glyph equality assertions.
- `exclude-from-sample NAME`
- `assume unused NAME...` — suppresses the unused-glyph warning (accepts patterns).

### Glyph blocks

`glyph NAME [W H] [flags...]` — flags: `sticky`, `inline`, `mark`, `advance N`, `left N`, `top N`,
`scale N`. `scale N` sets the per-glyph sub-pixel detail resolution (grid is N× finer).

- With `W H`: pixel rows follow immediately, 2 chars per pixel (`@@` filled, `..` empty, plus
  sub-pixel shape codes).
- `ref OTHER [COL ROW] [negated] [coloronly|monoonly] [fill COLOR]` — composite reference. Omitting
  the offset auto-resolves from `point`s. `fill` takes a `#RRGGBB[AA]` literal or a `color` name.
- `point POS COL ROW` (alias `anchor`) — anchor for auto-ref alignment; supports `+`/`-` prefixes
  and cell ranges.
- `glyph NAME [flags...] = ALIAS` — simple alias (single ref, no grid).
- `glyph NAME [flags...]` with no dims — ref-only composite, followed by `ref`/`point` lines.
- NAME supports the patterns above; blocks expand in lock-step with their `ref` patterns.

A glyph needs a pixel grid or at least one `ref` to exist at all — `advance`/`left`/`top`/`point`
do not make one buildable, and a contentless glyph never enters the resolution cache, so it is
absent from cmap, from composites and from GSUB coverage. Referencing one from a `map`, a `ref` or
a `remap` is an **error**; leaving it unused is only the usual unused-glyph warning. Pattern glyphs
are stricter still: they need `ref` lines, since a pixel grid cannot be shared across expansions.
For a deliberately blank glyph, `ref sp`.

### On-demand glyphs (`ref_composite.rs`)

Names that are not defined anywhere but match a synthesizable shape are generated on demand and are
**implicitly `inline`**:

- `WxH` — filled rectangle (W, H nonzero).
- `[-]A[pBrR]x[-]C[pDrR]` — fractional rectangle: dimension `A + B/R`. `R >= 2` and must match on both
  axes when both are fractional; `0 <= B,D < R`. A leading `-` right/bottom-aligns that axis within
  the cell. e.g. `1p2r3x4` = 1⅔ × 4.
- Any of the above with a `-ul`/`-ur`/`-dl`/`-dr` suffix — right triangle with legs W × H and the
  right angle at that corner (u = up, d = down).
- `X` where `X` is undefined but both `X:mono` and `X:color` exist — picks by rendering mode.

Sub-pixel shape codes live in `pixel.rs` (`PX_HALF*`, `PX_QUAD*`, `PX_SLANT*`, `PX_CONE*`,
`PX_CORNER*`, `PX_HQUAD`/`PX_VQUAD`, `PX_DOT`, …), each with a complement id via `^ PX_SUBPIXEL`.
`PX_CUSTOM` (25) is a sentinel meaning "geometry is a `DetailRegion` in the grid's detail table";
it only ever appears in derived grids and is **never serialized**.

## Testing

- `cargo test` — ~500 unit tests. Heaviest suites: `document_io.rs` (parser round-trips),
  `editor/view_tests.rs` (GUI scenarios), `render/ttf_builder.rs`, `editor/doc_links.rs`, `pattern.rs`.
- **GUI behavior must be tested through `EditorHarness` (`src/editor/harness.rs`)**, not left to manual
  testing. It drives the real `show_document` in a headless `egui::Context`, injects key/mouse events,
  and exposes per-frame layout snapshots (visual lines, grid rows, gutter numbers). Add new interaction
  tests to `src/editor/view_tests.rs` in that style. Past refactors "for testability" still left the
  frame loop untested, and scenario-level regressions (e.g. a grid demoted to text mid-header-edit)
  were invisible. Gotcha: synthetic clicks need time spacing or egui reads them as double-clicks — the
  harness handles this.
- Golden snapshots (`src/golden.rs`) cover the diagnostics report and a digest of what resolution
  produces over `testdata/`. Behaviour-preserving refactors must not move them; intentional changes
  update them so the diff is reviewable. Regenerate with `UNIFORM_UPDATE_GOLDEN=1 cargo test golden`.
- `assert` directives in `font/*.unf` are the font-level regression suite; run with `make test`.
  Prefer adding an `assert same/distinct` or `assert shape` when fixing a glyph-level bug.
- Per project policy: write the regression test first, observe the failure, then fix.

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

Ranked by how often recent commits touched them for a *fix* rather than a feature:

1. **`render/ttf_builder.rs` + `render/contour.rs`** — by far the most churn and the most fixes.
   Contour tracing over sub-pixel and on-demand shapes is the recurring theme: faulty rendering of
   fractional on-demand glyphs, wrong contours from glyphs containing on-demand triangle subglyphs,
   underestimated `xMax` when `coloronly`/`monoonly` layers are mixed, panics from multi-part shapes.
   Any change to shape codes, `DetailRegion`, or on-demand synthesis needs contour output checked at
   *composite* level, not just for the single new shape.
2. **`detail.rs` / `pixel.rs` geometry** — degenerate cases when merging two shapes, extremely tiny
   glyphs, zero-width pixel grids. Exact-rational sweeps make these correct-by-construction only if
   the degenerate inputs are actually handled; test empty/1×1/zero-extent inputs explicitly.
3. **`render/sample.rs` + `specimen.rs`** — the sample path resolves composites through its *own*
   cache values (shared driver in `glyph_cache.rs`). Bugs here are usually "font is right, sample is
   wrong": zero-width grids affecting layout, remap-only glyphs missing, color handling of indirectly
   mapped glyphs. When fixing a rendering bug, check both the TTF and the sample.
4. **`editor/document_view.rs` and the interaction layer** — focus capture, wheel scroll over the
   pixel grid, delete-key behavior, lost glyph flags after dragging a layer. These are exactly the
   regressions `EditorHarness` exists for.
5. **Name expansion and remap** (`pattern.rs`, GSUB in `ttf_builder.rs`) — empty remap targets,
   missing remap warnings, Hangul composition rules. The context-dependent parse rules above are the
   usual trap.
6. **Performance regressions count as bugs here** — a slow `resolve` used to snowball into dozens of
   concurrent rebuild threads. `UNIFORM_PERF`, the rebuild guard in `app.rs`, memoized exact
   subtraction and the `PixelGrid::rescale` caches all exist because of that. Keep the caches keyed
   correctly when changing geometry.

## Stack Overflow Monitor (`stackmon.rs`)

The main thread intermittently dies of a stack overflow on Windows with no reproducer; **the cause is
still unidentified as of 2026-07-26**. Static analysis (call-graph cycle detection over `src/`) found
no unbounded recursion in our own code, and `build.rs` already raises the Windows main stack to 8 MiB,
so the overflow exhausts 8 MiB. Evidence points at *within-frame* death: a Ctrl-C whose text never
reached the OS clipboard, because `ctx.copy_text()` only writes egui's output buffer and eframe applies
it at end of frame. Suspects not yet ruled out: egui tessellation, wgpu, the DirectWrite preview path
(`src/preview/directwrite.rs`).

**When the user reports another crash, ask for `uniform-stackmon.log` first — do not re-derive the
static analysis.**

`stackmon` is inert unless `UNIFORM_STACKMON=1`:

```sh
set UNIFORM_STACKMON=1
cargo xrr           # log goes to stderr and ./uniform-stackmon.log
```

On Windows a watchdog thread suspends the main thread every 250 ms and reads `Rsp`, so usage is
measured across *all* code (egui, wgpu, DirectWrite) with no instrumentation. Past
`UNIFORM_STACKMON_DUMP_PCT` (default 40%) of the stack it walks the suspended thread with
`StackWalkEx` and logs a symbolized backtrace — i.e. the runaway call chain is captured *before* the
process dies. A vectored exception handler also dumps `EXCEPTION_STACK_OVERFLOW` as a last resort
(`SetThreadStackGuarantee` reserves 128 KiB so it can run). One backtrace is logged at startup as a
self-test.

`crate::stackmon::phase("...")` markers in `app.rs::update` name the frame stage in each report.
`crate::stackmon::probe()` records a high-water mark where sampling is unavailable (non-Windows).
`[profile.release] debug = "line-tables-only"` exists so these backtraces have symbols; the `.pdb`
must sit next to the `.exe`.
