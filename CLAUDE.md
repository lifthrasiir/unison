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

Both run aliases go through `run-local.cmd`, which copies `uniform.exe` + `uniform.pdb` to
`%LOCALAPPDATA%\uniform\<profile>\` and runs *that* copy (working directory unchanged, so relative
arguments still resolve). **Never run the binary from the repo path**: the repo is an SMB mount, a PE
image is demand-paged from its file for the entire life of the process, and a cold page the share
cannot serve takes the process down with a stack overflow in ntdll — see *Stack Overflow Monitor*
below for the whole story.

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
| `UNIFORM_PERF` | `[perf]` per-stage timing logs for font/derived-data rebuilds (`app/background.rs`) |
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
  Tests live beside it in `document_io_tests.rs` (see *Where the tests live* below).
- `pattern.rs` — `NamePattern`, the single name-expansion engine (see *Name patterns* below).
- `pixel.rs` — `PixelShape`/`PixelGrid`, the shape-code catalog (`PX_*`), boolean ops, `rescale`.
- `detail.rs` — `DetailRegion`: exact per-pixel sub-pixel geometry on a `1/den` lattice, combined by
  an exact-rational trapezoid sweep. This is what makes composition exact instead of code-approximate.
- `ref_composite.rs` — composite (`ref`) resolution, anchor/point alignment, on-demand glyph synthesis
  (tests in `ref_composite_tests.rs`).
- `resolve.rs` — shared vocabulary for the resolution pipeline (`ItemRef` provenance, `Diagnostic`),
  so build/editor/validation cannot drift apart. Resolution emits issues directly.
- `issues.rs` — cross-document validation (missing refs, duplicate maps, unused glyphs, remap sanity).
- `script_run.rs` — script segmentation for shaping, mirroring browser behavior.
- `render/` — `contour.rs` (pixel shapes → contours), `glyph_cache.rs` (shared composite-resolution
  driver used by both `ttf_builder` and `sample`), `sample.rs` (sample HTML/PNG/live HTML), `assert.rs`
  (`assert` directives).
- `render/ttf_builder/` — contours → TrueType, GSUB, cmap; `UNITS_PER_EM = 1024`. `mod.rs` holds the
  collected-glyph vocabulary and the build entry points and delegates each stage:
  `expand.rs` (pattern expansion, on-demand/decomposed-map item synthesis),
  `collect.rs` (per-glyph refs, metrics, traced contours), `contours.rs` (the contour cache and
  `CachedContours`), `color.rs`, `gsub.rs`, `gpos.rs`, `hints.rs`, `outlines.rs` (glyf/metrics/cmap
  emission), `tables.rs` (final table assembly). Its tests are `render/ttf_tests/`.
- `stackmon.rs` — stack-overflow watchdog (below).
- `golden.rs` — `cfg(test)` golden snapshots over `testdata/`.

Editor (feature `editor`):

- `app/` — `UniformApp` eframe entry point; open documents, background font build, derived data
  (resolved glyphs, issues). Rebuilds are debounced (300 ms, 1000 ms after text input) and guarded
  against overlapping rebuild threads. `mod.rs` owns the struct, `new`, the `eframe::App` loop and
  the small shared helpers; `background.rs` (build/derive/assert threads and applying their results),
  `docs.rs` (open/flush/save/export), `history.rs` (go back/forward, below), `menus.rs`,
  `panels.rs`, `panes.rs` (the split-editor model,
  below), `rename.rs`, `search.rs` (the Search pane, below), `zoom.rs`.
- `editor/mod.rs` — `EditorState`, `EditMode` (`Normal` text editing, `GlyphEdit` pixel painting,
  `LayerMove` ref/layer repositioning). `editor/ids.rs` — `EditorId`/`Slot`, the per-instance `egui`
  id namespace (see *The editor is a widget* below).
- `editor/document_view/` — the editor widget: `DocumentEditor::show` and the `show_document` frame
  loop behind it (most churn in the editor). `mod.rs` is the
  loop itself plus the view cache; `layout.rs` (grid extents/strips, the visual-line model),
  `paint.rs` (the document area, selection, edit border, color backgrounds), `scroll.rs`, `keys.rs`,
  `popups.rs` (rename/autocomplete/error tooltip), `changes.rs` (writing edits back and re-deriving).
- `editor/` others — `caret`, `visual_lines`, `line_fields`, `doc_links`, `doc_input`, `editing`,
  `reconcile`, `undo`, `autocomplete`, `annotations`, `colors`, `minimap`, `inline_tools`,
  `glyph_widget`, `grid_render`, `pixel_interaction`, `pixel_selection`, `harness`, `view_tests`.
- `sidebar.rs` — `.unf` file list (open, rename, create). `specimen.rs` — specimen rendering.
- `edit_menu.rs`, `preview/` — bottom live-preview panel: rustybuzz shaping + platform rasterizer
  (`coretext.rs` on macOS, `directwrite.rs` on Windows).

`font/*.unf` are the font sources (one file per category). `testdata/` holds test-only `.unf` files
plus goldens. `data/` holds sample-generation inputs (confusables, UDHR text).

### The editor is a widget

An editor instance is `DocumentEditor { doc, lines, state, env }` and is shown with `.show(ui)`.
Those three `&mut` borrows are the *entire* instance state; `EditorEnv` is the borrowed, `Copy`,
read-only side (resolved glyphs, name parts, alternatives, color aliases, the two generation
counters, zoom, font id) that any number of editors share. Constructing a second `DocumentEditor`
over a second document and `EditorState` is all it takes to have two live editors in one frame —
`editor/view_tests.rs::two_editors_do_not_share_view_state` drives exactly that through
`EditorHarness::split`.

What makes that work is `editor/ids.rs`. Everything an editor parks in `ctx.data()` — scroll offset,
viewport height, scroll target, gesture zone, wheel acceleration, page-scroll request/sticky line,
grid hscroll drag, caret screen position, error-tooltip payload, hover flags, slant/layer/selection
drag accumulators — plus every named area, panel and interaction id, is keyed by the instance's
`EditorId`. `Slot` is the **single inventory** of those keys and is an enum on purpose: a new piece of
per-frame or cross-frame editor scratch is added there, never as a fresh `Id::new("...")` string.
`DocumentEditor::show` additionally wraps the body in `ui.push_id(state.id())`, so *auto-generated*
widget ids (the canvas, the scroll area, interaction rects) are salted too.

Two ids stay deliberately context-global, and both are documented where they live: the `Palette`
cache in `colors.rs` (derived from the context theme, identical for every editor) and
`debounced_scroll_step`'s coarse wheel cooldown in `scroll.rs` (it describes the input device — one
physical tick must yield one step no matter how many surfaces ask). Anything else global in the
editor is a bug.

The host still owns what is genuinely per-*pane*: zoom level, panel sizes and the zoom-routing
rects in `app/`, plus what is per-window (escape mode). Those are not editor state and do not
belong in `EditorState`.

### `app/panes.rs` — the split editor

The central area is one or two panes, split vertically; a *pane* is the whole editor surface (text,
grids, minimap, inline tool palette), not a sub-widget of one. `Panes` holds the list, the focused
index and the divider ratio; `Pane` holds the document index (`None` = the placeholder), the pane's
own zoom level and last frame's screen rect for zoom routing. Panes are views onto
`open_documents`, which is a buffer list: closing a pane detaches it from its document but leaves
the document open, dirty flag, undo stack and all — as already happened when a second file was
opened over a first.

Two invariants make the single sidebar unambiguous, and `panes.rs`'s tests pin both:

- **At most one placeholder.** An opened file goes to the placeholder pane if there is one, else to
  the focused pane. Two placeholders would leave no rule for "which one". The only operation that
  could produce a second is a split, so splitting is offered *only* from a single pane that has a
  document (`can_split`) — never from a placeholder, and never into a third pane.
- **A document is shown by at most one pane.** Opening a file already on screen just moves the
  focus to the pane showing it (`show_document`). Two live editors over one document would need
  line-by-line synchronization; that is deliberately not supported.

The focus follows whichever editor egui reports as focused (`sync_pane_focus`, run right after the
panes are laid out), and pane commands — Cmd/Ctrl+Alt+←/→ split, Cmd/Ctrl+Alt+H/L move the focus one pane left/right
(vim-keyed, since the arrows are taken and j/k stay free for a future horizontal split),
Cmd/Ctrl+Alt+X swap, Cmd/Ctrl+W close — are dispatched after that, so they act on the pane the focus is actually in this frame.
The swap chord is the exception to how accelerators are read: `egui-winit`'s `is_cut_command`
ignores alt and *returns* after pushing `Event::Cut`, so Cmd/Ctrl+Alt+X never arrives as a key
press at all. `intercept_swap_panes_chord` takes that event out of the queue at the top of the
frame (beside `intercept_hex_codepoint_input`, the other input-queue rewrite) — left in, the
focused editor cuts the selection instead.
Dragging the divider onto either edge and releasing closes the pane it collapsed; `split_layout`
still clamps both panes to `MIN_PANE_WIDTH`, so the shaded overlay is what tells the user the drop
will close rather than resize.

### `app/history.rs` — go back / go forward

The reverse of Ctrl/Cmd+click "go to symbol" (Cmd/Ctrl+T back, Cmd/Ctrl+Shift+T forward; Edit menu).
The stack behaves exactly like the undo stack — pushing while rewound replaces everything after the
current position — but each entry stores **two** positions, because following a link is asymmetric:
Go Back must reach the *link*, Go Forward the *target*. One position per entry lands one of the two
directions a step off.

`from` is the link's own line/column, not the caret — a Ctrl+click deliberately leaves the caret
alone, so a `from` taken from the caret points wherever the user last clicked
(`view_tests.rs::following_a_link_reports_the_link_position_not_the_caret` pins this).

The history spans files, so it lives in `UniformApp`, not in an `EditorState`; locations are indices
into `open_documents`, which is only ever appended to, so an index cannot come to mean a different
file. Opening another folder clears that list and clears the history with it. The editor reports
each followed link once, as `DocumentViewResult::nav` — a `NavTarget::Local` the editor already
carried out itself, a `NavTarget::CrossFile` only the host can resolve, or a `NavTarget::Search`
(below) — so every case is recorded through one path.

Positions are **not** rewritten when the document is edited under them, so a jump remembered across
an insertion can come back a few lines off (navigation clamps, so it is stale, never invalid). Doing
better needs anchors that every mutation updates: `UndoStack::push_lines` is nearly the choke point
for line-count changes, but `reconcile.rs` and `rename.rs` bypass it, so it is not one edit yet.

### `app/search.rs` — the Search pane

The fourth bottom-panel tab, listing every place a name is written in the diagnostics list's format.
It is **where a Ctrl/Cmd+click goes when "go to definition" cannot answer**, which is two situations
that deliberately share one destination: the token clicked *is* the declaration
(`NavTarget::Search`), or it refers to a name nothing declares (`NavTarget::CrossFile` that
`goto_glyph` fails to resolve). So clicking `glyph foo`'s own name lists its uses, and clicking a
`ref` with a typo in it lists everyone who shares the typo, instead of the click doing nothing.

For that to work, `doc_links::extract_line_links` emits links for **definitions** too, flagged
`is_def` — the flag is what stops the editor from "navigating" to the line the click was already on.
Two `LinkTargetKind`s are search-only: `Anchor` and `Feature` have no declaration site at all (an
anchor is matched by name across glyphs, a feature tag is declared once per target), so they never
navigate. A *pattern* glyph name gets no definition link: it is not a name anything can refer to,
and only the `$var`s inside it are.

Matching goes through `line_fields` like everything else that reads names, which is what keeps the
namespaces apart — a `remap` group named `liga` is not a hit for a glyph named `liga`. An anchor is
listed through **both** signs, since `+above` and `-above` are two sides of one anchor.

A hit is addressed by its **ordinal within its file**, not by its line number: opening a file
canonicalizes its text, so the line a hit sits at on disk need not be the line it lands on in the
editor. Canonicalization rewrites spacing and comments, never the order names appear in. The ordinal
counts *occurrences*, not lines — a line naming the same glyph twice is two rows, and both ends have
to agree on that or every later hit in the file lands one off.

`match_spans` returns the written token's whole span, which the pane highlights so a long `remap` or
`assert` row says where on it the name is. Two `contains(name)` pre-filters in front of it are what
keep a search a click and not a wait — one per file, one per line — and they are sound because every
kind's name occurs **literally** in the source (a name-parts name carries its own `$`, an anchor's
sign only precedes it). The per-line one matters most: a font directory is mostly pixel rows, and
classifying a line costs a tokenizing pass (`font/`: 9.9 ms → 1.7 ms). Unopened files are then
served by `file_text`, cached on **mtime** — a closed file changes only from outside the editor,
which is exactly what a generation counter would not see. Open documents are searched as they stand,
unsaved edits included.

Clicking a hit records a history entry whose `from` is the **caret** — unlike a link, the pane is
not a position in a document, so the caret is the only place the user can be said to have left.

### `line_fields.rs` — where names live

`LineField`/`FieldRole` is the *single* place that knows which tokens on a line name an entity
(`GlyphDef`, `GlyphRef`, `NamePartsDef/Value`, `PointDef`, `ColorDef/Ref`, `RemapGroupDef/Ref`,
`FeatureDef`). Clickable links, rename detection, rename mutation, completion of existing tokens and
the Search pane all consume it. **Adding a new directive form means describing it once here**, not in
five features. (What completion offers *between* tokens is a separate concern and stays in
`autocomplete.rs`.)

Two distinctions the roles draw are easy to lose: a `remap` line's first operand names the *group*,
not a glyph (so a glyph rename must not rewrite it), and `feature ... : anchor NAME` names an anchor
where the plain form names a remap group — the `anchor` keyword is the only thing telling them
apart.

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
spaces; four backticks (two to escape, two to quote) for a literal backtick.

**Comments.** `//` starts a comment on *every* line except pixel rows (where `//` is a legal pixel
pair, so `split_comment` must never see one). It is a single token that runs to the end of the line
and cannot be quoted, so ``foo `//` bar // quux`` is four tokens: the quoted `` `//` `` is ordinary,
the unquoted one opens the comment. `document_io::split_comment` is the single implementation;
`tokenize_tokens`/`tokenize_with_spans` drop the comment, so grammar, links, completion and rename
never see comment prose. Every item keeps its comment (`comment` field on the structured
`DocumentItem` variants, on `GlyphBody`/`GlyphRef`/`GlyphPoint`, and inline in the raw text of
`FontMeta`/`Directive`) so serializing a document does not lose it — the editor canonicalizes each
file through `serialize_document` on open, so a comment the model drops is a comment the user loses.
Appending to a line must go through `append_to_line` to stay in front of the comment.

### Directives

- `font-meta height H ascent A descent D`
- `map CHAR = GLYPH` — cmap mapping.
- `map generate CHAR [= GLYPH]` — cmap mapping to a glyph synthesized from the character's Unicode
  canonical decomposition, named `uniXXXX` unless `GLYPH` names it. `GLYPH` is a pattern expanded in
  lock-step with `CHAR`, exactly as a plain `map`'s target is. The `generate` keyword is mandatory:
  the older bare `map CHAR` was too easily misread as the plain form. The synthesized refs carry
  `inherit` implicitly (the composite stands in for its decomposition, so it exposes the surviving
  anchors — see *Anchor exposure is opt-in* under Glyph blocks); hand-rewriting one as a plain
  `glyph` + `map` means deciding per ref whether to keep `inherit`.
- `name-parts $NAME = token1 token2 ...`
- `color NAME = #RRGGBB[AA] [coloronly|monoonly]` — named palette entry.
- `remap FEATURE : [LOOKBEHIND... :] SOURCE... -> TARGET... [: LOOKAHEAD...]` — GSUB substitution.
  Source and target are *lists* of glyph names in all cases; an empty target means removal.
  The list lengths pick the lookup type: 1→1 single, 1→N (incl. 1→0) multiple, N→1 ligature.
  N→M and N→0 have no OpenType lookup type and are an **error** — `issues.rs` reports them rather
  than letting the builder emit something close-but-wrong.
- `feature NAME for TARGET... : REMAP_GROUP` — OpenType feature. A target is a script tag (`latn`,
  `DFLT`) or a script narrowed to one language system, `script/LANG` (`latn/ROM`). The two are
  written explicitly rather than told apart by their tag: the registries' one apparent collision is
  inverted (`DFLT` is the default *script*, `dflt` the default *language*), and a language tag is
  meaningless without its script (`SRB` lives under both `latn` and `cyrl`). Directives sharing a
  tag *and* a target are merged into one feature record (lookups accumulate in declaration order),
  because a shaper only ever finds the first record for a tag. Same tag under different targets
  stays separate.
- `feature NAME for TARGET... : anchor ANCHOR_NAME` — anchor-driven (mark attachment) variant.

  **Both fallbacks are replacements, not extensions**, and `gsub.rs`'s `inherit_tags` exists for
  exactly that: a shaper reads `DFLT` only when the script it wants has no record at all, and reads
  a `LangSys` *instead of* its script's default. So the builder folds `DFLT`'s features into every
  declared script, and each script's default into every language below it, merging per feature tag
  so an inherited tag and a redeclared one end up as one record. Left out, adding a single
  `locl for latn/ROM` silently costs all Latin text its `ccmp` — and every mark attachment with it.
- `assert shape TEXT [@lang] [+feat|-feat...] : GLYPH [advance N] [offset X Y] : GLYPH ...` —
  shaping assertion. `@lang` is a **BCP 47** tag (`@ro`), not the `script/LANG` a `feature` takes:
  an assertion states the input a real client hands the shaper, and the OpenType language system is
  what the shaper must derive from it. Writing `@ROM` on both sides would make them agree by
  construction and stop the assertion from noticing that Romanian never reaches the declared tag.
- `assert same NAME...` / `assert distinct NAME...` — resolved-glyph equality assertions.
- `exclude-from-sample NAME`
- `assume unused NAME...` — suppresses the unused-glyph warning (accepts patterns).

### Glyph blocks

`glyph NAME [W H] [flags...]` — flags: `sticky`, `inline`, `mark`, `advance N`, `left N`, `top N`,
`scale N`. `scale N` sets the per-glyph sub-pixel detail resolution (grid is N× finer).

- With `W H`: pixel rows follow immediately, 2 chars per pixel (`@@` filled, `..` empty, plus
  sub-pixel shape codes).
- `ref OTHER [COL ROW] [negated] [inherit] [coloronly|monoonly] [fill COLOR]` — composite reference.
  Omitting the offset auto-resolves from `point`s. `fill` takes a `#RRGGBB[AA]` literal or a `color`
  name.
- `point POS COL ROW` (alias `anchor`) — anchor for auto-ref alignment; supports `+`/`-` prefixes
  and cell ranges.
- `glyph NAME [flags...] = ALIAS` — simple alias (single ref, no grid; carries no ref flags, so an
  alias that must forward its target's anchors is written in block form with `ref TARGET inherit`).
- `glyph NAME [flags...]` with no dims — ref-only composite, followed by `ref`/`point` lines.
- NAME supports the patterns above; blocks expand in lock-step with their `ref` patterns.

**Anchor exposure is opt-in** (`derive_ref_offsets_with`). Inside a composite, a ref's `-name`
anchors attach to a *unique* size-matching `+name` published by a sibling (or declared by the
composite), consuming it; the ref's own `+` anchors are then published — all regardless of flags.
What the composite *exposes* to the outside (GPOS base anchors, the anchor shadow, further
composition) is only its own declared anchors plus the surviving anchors of refs marked `inherit`.
So a digraph or a circled letter exposes nothing it did not say, and `glyph i-lower`'s
`ref i-lower:dotless inherit` is what lets Ï build on it. `map generate` composites stand in for
their decomposition, so their synthesized refs inherit implicitly — hand-rewriting one as a plain
`glyph` + `map` means deciding per ref whether to keep `inherit`. Two rules are load-bearing and
**loud** (`issues.rs` errors, via the anchors-only pass sharing `glyph_cache`'s driver): an exposed
set containing the same anchor name twice exposes *neither* (declare it explicitly instead), and a
`-` anchor with more than one size-matching `+` candidate attaches to *nothing*. A minus anchor no
remaining ref can satisfy does not defer its ref — deferral would let explicit-offset siblings
commit first and miss their consumption.

A **negative `ref` offset is a bearing, not something to normalize away**: the glyph origin stays at
(0, 0), the outline keeps its negative coordinates (negative lsb, or ink above the ascent), and the
advance still measures only the extent to the *right* of the origin. `left`/`top` are for shifting a
glyph that has no such ref; do not use them to undo a negative offset. Every composite path has to
agree on this — `CachedContours`/`CachedGlyph` therefore carry `origin_row`/`origin_col` (the logical
coordinate of raster cell (0, 0)) beside their normalized grid, and a parent adds a ref target's
origin to the `ref` offset when placing it, or it silently loses whatever sits left of that origin.
`contour::track_contour_multi{,_diff}` normalize to the bounding box; the production callers use the
`_at` variants, which put the contours back in the layers' own space.

A bearing only exists where something is drawn: `glyph_cache::trim_blank_before_origin` trims the
blank margin before the origin and pulls `origin_*` back towards zero, so pulling a ref up into its
own empty top rows (`ref X 0 -3`) stays metrically identical to placing that ink directly. Without
it such a glyph grows a phantom bearing that the sample then pads its cell for.

### The metrics overlay (View ▸ Show glyph metrics)

The editor draws each glyph's metric box over its grid. `left`/`top` move the **ink**, so in grid
coordinates the box sits at `-left` / `-top`; its width is `advance` (falling back to the resolved
extent right of the origin) and its bottom is computed rather than written — hence no flag for it.
Inside it, a second left/right-open box runs from the ascent line (the box's own top) down to the
baseline at `-top + ascent`. `GridExtent::include_metrics` widens the drawn area to the box, which
is what makes a two-row mark like `dia-below` show where on the line it actually lands.

`bottom` is clamped **both ways**: `min(resolved height, -top + ascent + descent)`. The em box is
the upper bound, but a glyph shorter than it has no cell below its own last row either — bound only
by `font-meta height`, a one-row glyph was drawn as sixteen rows of grid.

Everything the box takes from `left`/`top`/`advance`/`font-meta` is in **logical pixels**, while the
grid of a `scale N` glyph is in subcells (`document_io` multiplies the declared dimensions but not
the flags), so `glyph_metrics` scales the former group and nothing else — same split
`ttf_builder::collect` works in.

The baseline pair is drawn on **any glyph tall enough to reach it** (`resolved_h > ascent`; at
`ascent 14`, fifteen rows), regardless of whether anything maps the glyph. Gating it on cmap/GSUB
reachability was tried and reverted: a glyph is normally drawn before it is mapped, and a `flags`
glyph is reached through its own `:mono`/`:color` variants and never mapped at all, so metrics that
wait for a `map` line are metrics you cannot design against.

Each metric line is three 1 px strokes — a `grid_bg` band between two `grid_on` ones, the baseline
pair additionally dashed — **inset inward** and sized in points, the one thing in the editor that
ignores the zoom level, because a band that grew with the cells would read as another sub-pixel
shape. Insetting is load-bearing: the drawn area is widened to exactly the box, so a centred stack
would lose its outer half to the clip on every flush edge. The outer box is drawn as three *closed
rectangles* (background ring first, then both border rings), never as four edges: edge by edge,
whichever of the two meeting at a corner comes second lays its background band over the other's
border stroke and breaks it.

A glyph needs a pixel grid or at least one `ref` to exist at all — `advance`/`left`/`top`/`point`
do not make one buildable, and a contentless glyph never enters the resolution cache, so it is
absent from cmap, from composites and from GSUB coverage. Referencing one from a `map`, a `ref` or
a `remap` is an **error**; leaving it unused is only the usual unused-glyph warning. Pattern glyphs
are stricter still: they need `ref` lines, since a pixel grid cannot be shared across expansions.
For a deliberately blank glyph, `ref sp`.

### The anchor shadow (`editor/anchor_shadow.rs`)

Selecting an `anchor` layer in the subglyph palette draws every glyph that could attach there under
the glyph being edited. Attachment is symmetric, so the two signs are **not** told apart: a `+above`
shadows the marks carrying `-above` and a `-above` shadows the bases carrying `+above`, found the
same way and subject to the same `size_matches` rule composition applies. What is drawn is the
*union* of all of them — a cell is inked when any candidate inks it, and its geometry is the exact
union of every candidate's, so edge cells routinely become `PX_CUSTOM` details.

Placement mirrors composition exactly (`try_match_minus_plus`'s anchor delta, which is *not*
scale-converted, plus `ref_effective_offset_scaled`'s scale-converted origin), so the shadow lands
where the glyph really would. `GridExtent::include_shadow` then widens the drawn area to it, for the
same reason `include_metrics` does: a two-row mark otherwise shows none of the base it lands on.
The shadow is part of `ViewData`, so `ViewCacheKey` carries the selected *anchor* layer — ref layers
are deliberately left out of the key, since cycling through them changes nothing the view is built
from and rebuilding it is O(document).

The union does not go through `PixelGrid::blit`: a shadow is every attachable glyph at once, so its
cells overlap far more than a composite's, and `anchor_shadow::union_into` takes the two cases that
dominate (destination already a full inked pixel, or the same shape twice) before the exact sweep.

### On-demand glyphs (`ref_composite.rs`)

Names that are not defined anywhere but match a synthesizable shape are generated on demand and are
**implicitly `inline`**:

- `WxH` — filled rectangle (W, H nonzero).
- `[-]A[pBrR]x[-]C[pDrR]` — fractional rectangle: dimension `A + B/R`. `R >= 2` and must match on both
  axes when both are fractional; `0 <= B,D < R`. A leading `-` right/bottom-aligns that axis within
  the cell. e.g. `1p2r3x4` = 1⅔ × 4.
- Any of the above with a `-ul`/`-ur`/`-dl`/`-dr` suffix — right triangle with legs W × H and the
  right angle at that corner (u = up, d = down).
- Any of the above with a trailing `:ceil`/`:floor`/`:zero` — the bitmap fill rule (below).
- `X` where `X` is undefined but both `X:mono` and `X:color` exist — picks by rendering mode.

Any other `:`-suffix makes the name a non-match, so ordinary glyph names containing a colon fall
through to normal lookup.

**The bitmap fill rule (`BitmapFill`).** The font is built twice — a vector build that reads the
geometry and a bitmap build that keeps only the `PX_FULL` ink flag and squares every lit cell off
(`ttf_builder::contours::CachedContours::from_grid`). A synthesized shape therefore has to decide which cells
that second build lights, and it does so **per logical pixel** (not per subcell) from the exact
covered area: `Round` (default, ties round up), `:ceil` (any coverage), `:floor` (full coverage
only), `:zero` (never — vector-only). Whole-pixel shapes are covered 1/1 everywhere, so `WxH` names
render identically under everything but `:zero`.

Two invariants hold this together and are easy to break:

- **Per logical pixel, applied uniformly to that pixel's subcells.** `PixelGrid::rescale` ORs the ink
  flags of the source subcells each destination subcell covers, and it preserves logical dimensions,
  so that OR never crosses a logical pixel boundary. Deciding per subcell instead gets undone by that
  OR, which is itself a `Ceil`.
- **The rule moves no outline.** `make_on_demand_grid` lays down geometry first and stamps ink flags
  afterwards (`apply_bitmap_fill`), so the vector build cannot observe the flag.

The ½ tie is real (it is every 45° triangle edge cell), so the area comparison must stay exact —
`DetailRegion::area_exact` returns `(num, den)` on the lattice for that reason. `area2` is the f64
test helper, not the production path.

Sub-pixel shape codes live in `pixel.rs` (`PX_HALF*`, `PX_QUAD*`, `PX_SLANT*`, `PX_CONE*`,
`PX_CORNER*`, `PX_HQUAD`/`PX_VQUAD`, `PX_DOT`, …), each with a complement id via `^ PX_SUBPIXEL`.
`PX_CUSTOM` (25) is a sentinel meaning "geometry is a `DetailRegion` in the grid's detail table";
it only ever appears in derived grids and is **never serialized**.

## Testing

- `cargo test` — ~680 unit tests. Heaviest suites: `document_io_tests.rs` (parser round-trips),
  `editor/view_tests.rs` (GUI scenarios), `render/ttf_tests/`, `editor/doc_links.rs`, `pattern.rs`.
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

### Where the tests live

Small `#[cfg(test)] mod tests` blocks stay at the bottom of the module they test. Where the suite grew
past the source it tests, it lives in a sibling file (or directory) declared as a *child* module through
`#[path]`, so it still reaches the module's private items:

| Module | Tests |
| --- | --- |
| `render/ttf_builder/` | `render/ttf_tests/` — `misc`, `hints`, `gsub`, `gpos`, `color`, `composite`, with shared canonicalization helpers in its `mod.rs` |
| `document_io.rs` | `document_io_tests.rs` |
| `ref_composite.rs` | `ref_composite_tests.rs` |
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

Ranked by how often recent commits touched them for a *fix* rather than a feature:

1. **`render/ttf_builder/` + `render/contour.rs`** — by far the most churn and the most fixes.
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
4. **`editor/document_view/` and the interaction layer** — focus capture, wheel scroll over the
   pixel grid, delete-key behavior, lost glyph flags after dragging a layer. These are exactly the
   regressions `EditorHarness` exists for.
5. **Name expansion and remap** (`pattern.rs`, `ttf_builder/gsub.rs`) — empty remap targets,
   missing remap warnings, Hangul composition rules. The context-dependent parse rules above are the
   usual trap.
6. **Performance regressions count as bugs here** — a slow `resolve` used to snowball into dozens of
   concurrent rebuild threads. `UNIFORM_PERF`, the rebuild guard in `app/background.rs`, memoized exact
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

A captured overflow (2026-07-29, idle window) narrowed it: of 8155 frames, **exactly one was outside
ntdll**, and the rest were a 4-frame, 3.9 KiB cycle repeated ~2000 times — an exception being raised,
and its dispatch faulting and raising again, not recursion in our code. The growth decelerates
(2.32 → 1.34 → 1.07 → 0.73 MiB per 250 ms tick), which is the quadratic cost of each nested dispatch
walking an ever-deeper stack. `phase=update:central/editor` puts the *first* exception between
`app/mod.rs:403` and `:450`, i.e. in the central panel/editor, not in tessellation. The walk cannot
reach past the storm, so the originating frame is invisible — which is why the handler now records
first-chance exceptions (below): the culprit is the exception, not the stack.

A second capture (2026-07-29, on Alt-F4) got the culprit, because the exception recorder was in by
then. The storm is preceded by **one** exception in our own image — `0xc0000006`
(`STATUS_IN_PAGE_ERROR`) at `uniform+0x76ea0`, which `llvm-symbolizer --obj=uniform.exe 0x140076ea0`
(RVA plus the `0x140000000` image base) resolves to `UniformApp::confirm_close_and_maybe_save`
(`app/docs.rs`), the Alt-F4 save-confirmation dialog. Everything after it is the same 4-frame ntdll
cycle, now explained: `KiUserExceptionDispatcher` has to unwind, unwinding reads `.pdata`/`.xdata`
from another not-yet-resident page of the *same* image, that read faults the same way, and each
nested dispatch does it again.

So this is not recursion, and not a bug in the code at the faulting address — a fault on an
already-mapped image page means Windows could not read that page back from the file. **The `.exe` is
run over SMB**: this is a cross-build (`cargo xb -r` on macOS, `cargo xrr` on Windows over the same
repo path via a Samba mount), so a 16 MB image is demand-paged over the wire, and the PE image is
*not* loaded up front — a cold page is read from the file the first time it is executed. That is why
every report lands in cold code (the close dialog; teardown `drop_in_place`/`mpmc::list::Channel::drop`
glue in the 2026-07-30 capture) and never in resident paths, and why it needs no memory pressure.

**Reproduced 2026-07-30** by truncating the image in place while the app ran
(`truncate -s 0 …/release/uniform.exe` on the server, then Alt-F4 ▸ Don't Save): overflow every time.
Two things that repro settled:

- The exception code is *not* the invariant — that run's storm ran on `0xc0000005`
  (`read of 0xfffffffffffffffe`) raised **inside `RtlVirtualUnwind`** rather than `0xc0000006` in our
  own code. Unreadable `.pdata`/`.xdata` makes the unwinder itself fault, which is the same storm from
  one step earlier. Match on the *shape* (one exception outside ntdll, then a 4–5 frame ntdll cycle),
  not on the code.
- **Deleting the file on the server cannot reproduce it**, and neither can a rebuild. POSIX `unlink`
  only drops the directory entry; `smbd` holds the file open for the life of the image section, so the
  inode and its data stay readable. And `cargo` does not write the final path in place — it re-links
  `deps/uniform-<hash>.exe` into it, so a rebuild yields a **new inode** (verified: 18356881 →
  18356974) and the mapped one survives untouched.

What is left as the real-world trigger is **loss of the SMB session backing the section** (server
sleep, `smbd` restart, network drop, `deadtime`), possibly a lease break after a rebuild reopening by
path onto the new inode — plausible, not verified. The fix is not in the code: run the `.exe` from a
local NTFS disk. `fault_detail` now prints the paging `NTSTATUS` for `0xc0000006`
(`[read of 0x… failed with 0x… (unexpected network error)]`), which names the cause in one line.

**When the user reports another crash, ask for `uniform-stackmon.log` first — do not re-derive the
static analysis.** Symbolize any `uniform+0xRVA` against the `.pdb` of that build before theorizing —
`llvm-symbolizer --obj=uniform.exe --demangle <RVA + 0x140000000>` works from macOS, and `cargo xb -r`
re-links the same bytes from `deps/` when nothing changed, so a truncated `.exe` can be restored
without invalidating the symbols.

**Ahead of deleting `stackmon`:** the cause above is environmental, so once `run-local.cmd` has been
in use for a while with no overflow, this module and its `phase()` markers are dead weight and should
go. It has not been observed silent yet, which is the only reason it is still here.

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

That handler also **records every first-chance exception** — code, faulting address, thread, phase —
as `exception 0xc0000005 (access violation) x1234 (+600) at ... phase=...`, one line per distinct
`(code, address)` with a running count, so an exception storm shows up as a count that explodes.
Recording runs at the fault point, on any thread, possibly under the heap lock: it is a fixed
32-entry table of atomics with no allocation, no logging and no blocking, and the *watchdog* turns it
into log lines. `log` carries a thread-local reentrancy guard for the same reason (a nested `log`
would deadlock on its own non-reentrant mutex).

Every multi-line report — the exception list, the module list, a backtrace *with its header* — goes
out through `log_block`/`log_lines`, which takes the lock **once** and writes one buffer. Line by line
it does not survive contact with the second thread: the watchdog ticks every 250 ms while the handler
logs from the main thread, and the 2026-07-30 capture has a `used 4.45 MiB` line sitting between two
frames of the overflow backtrace. Nothing is lost by buffering, since every walk already finishes
before its first line is formatted. Module load bases are logged as they appear
(`log_new_modules`), and an unsymbolized frame prints as `module+0xRVA` — with ASLR, a bare address
is unusable after the fact.

`crate::stackmon::phase("...")` markers in `app::update` name the frame stage in each report.
`crate::stackmon::probe()` records a high-water mark where sampling is unavailable (non-Windows).
`[profile.release] debug = "line-tables-only"` exists so these backtraces have symbols; the `.pdb`
must sit next to the `.exe`.
