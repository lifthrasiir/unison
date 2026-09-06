# Uniform

Bitmap font editor with sub-pixel shape support and a TTF builder, plus the `font/` sources of the
Unison font itself. egui/eframe GUI, Rust 2024 edition. One binary `uniform`: the GUI by default,
and the headless `build`, `test`, `fix`, `probe` and `sequences` subcommands.

**This file is an index, and it is loaded into every session — keep it short.** The reasoning behind
each design lives in the module-level `//!` docs of the code that implements it; when a new
invariant is worth recording, put it next to the code. `doc/internals.md` is the topic → module
index for those docs; add a row there, not here. User-facing documentation is under `doc/`.

## Build & Run

```sh
cargo build -r    # normal build
cargo test        # unit + golden + GUI-harness tests
make              # build unison.ttc / unison-%.woff2 + demo.html
make test         # the above, `cargo test --no-default-features`, then the `assert` directives in font/
```

The GUI takes an optional font directory: `cargo run -r -- font/`. The subcommands and their flags
are in `doc/reference.md` (*Font Project*); the ones used most:

```sh
cargo run -r -- build -i font/ -o unison.ttc [-o unison-%.woff2] [--demo-html demo.html -d data]
cargo run -r -- test -i font/
cargo run -r -- fix -i font/ --optimize-clearance [--dry-run]
```

`build` and `test` print parse errors, then the `issues/` report (`error:`/`warning:` with
`file:line:`). Warnings still build; a single `error:` exits 1 — `build` after writing every
output, which is what CI relies on (`.github/workflows/pages.yml`). Read the report.

Cross-compiling for Windows when the current environment is not Windows: `cargo xb -r` builds,
`cargo xrr` / `cargo xr` run through `run-local.cmd`, which copies the executable off the SMB mount
first. **Never run the binary from the repo path** — its comments say why.

`UNIFORM_PERF` prints `[perf]` per-stage timings in every mode; `UNIFORM_UPDATE_GOLDEN=1` rewrites
`testdata/*.golden`; `UNIFORM_WATCH_POLL_MS` and `UNIFORM_PROFILE_RUNS` are documented where they are
read (`app/watch.rs`, `ref_composite/`).

## Rules that are not visible from any one file

- **`font/` is a consumer, not a fixture.** No automated test may read it: it changes for
  font-design reasons and is far too large to be meaningful. When `font/` turns up a bug, add a
  minimal `.unf` to `testdata/` or build the case inline. Manual runs against it (`make test`,
  `cargo run -r -- build -i font/`) are expected. The one `#[ignore]`d profiling harness in
  `ref_composite/` is the exception; keep any such case `#[ignore]`d.
- **GUI behaviour is tested through `EditorHarness`** (`editor/harness.rs`), never left to manual
  testing. Scenarios go in `editor/view_tests/`, one module per theme.
- **Regression tests first**: write the test, observe the failure, fix, observe the pass. Prefer an
  `assert same/distinct/shape` in `font/*.unf` for a glyph-level bug.
- **Goldens** (`golden.rs`, over `testdata/`) pin the diagnostics report and a digest of resolution.
  Behaviour-preserving refactors must not move them; intentional changes update them so the diff
  is reviewable.
- **The headless build rots silently.** Most of the crate is under `#[cfg(feature = "editor")]`
  and `cargo test` never builds without it, so `make test` runs `cargo test --no-default-features`.
  An item only the headless *binary* does not need stays `#[cfg(feature = "editor")]`, and so does
  a test that reaches for it. An item live in the headless test build but dead in the headless
  binary takes `#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]` — `expect`,
  so that it fails once the item does get used there.
- **Performance regressions are bugs.** The build's expensive stages run on every core
  (`parallel.rs`), so shared mutable state added to one of them is the bug to avoid: a memo a
  stage carries sits on the serial side of the split. The editor is routinely run against a
  network share where one file round trip is ~185 ms, so **nothing on the UI thread reads a
  directory file by file or builds a font** (`startup.rs` measures it, `app/background.rs` is
  where the work goes). Keep the geometry caches keyed correctly when changing geometry.
- **Files stay around 2000 lines.** Split by stage (as `ttf_builder/` and `document_view/` are).
  A test suite that outgrows its module lives in a sibling file or directory declared as a *child*
  module through `#[path]`, so it still reaches private items; the table below says which.
- **The release profile is tuned for binary size**, and `Cargo.toml`'s own comments are the
  reference. The rule it is held to: a warm rebuild of this crate must not get slower.

## Source layout

Core (feature-independent):

| Module | What it holds |
| --- | --- |
| `document/`, `document_io.rs` | The `.unf` data model, parser and serializer. `document_io.rs` is where the syntax the parser reads is spelled out; `doc/reference.md` is the user-facing reference. |
| `pattern.rs`, `exists.rs`, `alias.rs`, `merge.rs` | Name expansion, the `exists` search, `glyph A = B`, and implicit merges of one pattern block. |
| `pixel.rs`, `detail.rs`, `on_demand.rs`, `math.rs` | Shape codes, exact sub-pixel geometry, synthesized shapes, gcd. |
| `ref_composite/`, `compose.rs` | Composite (`ref`) resolution and the IDC lines. |
| `faces.rs`, `meta.rs`, `audit.rs`, `samples.rs`, `ucd.rs` | The non-glyph directives: faces/slices, `meta`, `audit`, `sample`, `prop`. |
| `resolve.rs`, `glyph_flags.rs`, `issues/` | Diagnostics: shared vocabulary, per-glyph flags, and the cross-document checks (one module per check). |
| `fix/` | `uniform fix`: the commands that rewrite the source. |
| `parallel.rs`, `cancel.rs`, `startup.rs` | The work-stealing loop, cancellation, and the startup timeline. |
| `render/` | `contour.rs` (shapes → contours), `glyph_cache.rs` (the resolution driver the build and the specimen share), `ttf_builder/` (contours → TrueType/GSUB/GPOS/cmap, one submodule per stage), `assert.rs`, `sample.rs`, `reach.rs`, `demo/` (`demo.html`). |
| `script_run.rs`, `golden.rs` | UAX #24 itemization; golden snapshots. |

Editor (feature `editor`):

| Module | What it holds |
| --- | --- |
| `app/` | `UniformApp`: the frame loop, background rebuilds, documents, panes, history, search, rename, resize, save, watch, settings, timing. |
| `editor/mod.rs`, `editor/ids.rs` | `EditorState`, `EditMode`, and the editor-is-a-widget model. |
| `editor/document_view/` | The editor widget's frame loop, split by concern (`layout`, `paint`, `scroll`, `keys`, `popups`, `changes`). Most churn is here. |
| `editor/` others | One file per feature: shadows, caret, popups, folding, comment toggle, resize, annotations, autocomplete, links, `line_fields` (the one place that knows where names live on a line), `harness`, `view_tests/`. |
| `sidebar.rs`, `specimen.rs`, `edit_menu.rs`, `preview/` | File list, the specimen panel, the bottom live preview with its three shaping backends. |

`font/*.unf` are the font sources. `testdata/` is test-only `.unf` plus goldens. `data/` holds the
sample-generation inputs read through `-d data`, and `Blocks-17.0.0.txt`, the one file compiled in
(`ucd.rs`). `data/ref/` is untracked drawing reference cut by `scripts/extract_ref_charts.py`.

## Tests

`cargo test` is ~1500 tests. Small `#[cfg(test)] mod tests` blocks stay at the bottom of their
module; the suites that outgrew one:

| Module | Tests |
| --- | --- |
| `render/ttf_builder/` | `render/ttf_tests/` (shared helpers in its `mod.rs`) |
| `document_io.rs` | `document_io_tests/` |
| `document/` | `document/document_tests.rs` |
| `issues/` | `issues/issues_tests.rs` |
| `ref_composite/` | `ref_composite/ref_composite_tests.rs` |
| `editor/document_view/` | `document_view/tests.rs` (helpers) and `editor/view_tests/` (harness scenarios) |
| `exists.rs`, `pixel.rs`, `specimen.rs`, `meta.rs`, `faces.rs`, `on_demand.rs`, `compose.rs`, `fix/clearance.rs`, `render/sample.rs`, `render/reach.rs`, `editor/pixel_selection.rs`, `editor/ref_images.rs`, `editor/glyph_resize.rs` | `<name>_tests.rs` beside the module |

## Where the bugs come from

Ranked by how often a commit touched them for a fix rather than a feature:

1. `render/ttf_builder/` + `render/contour.rs` — contour tracing over sub-pixel and on-demand
   shapes, seen through a composite rather than alone. Check contour output at composite level.
2. `detail.rs` / `pixel.rs` — degenerate inputs (empty, 1×1, zero extent). Test them explicitly.
3. `specimen.rs` + `render/demo/` — "the font is right, the specimen is wrong". Check both.
4. `editor/document_view/` and the interaction layer — focus, wheel routing, delete, lost flags
   after a drag: exactly what `EditorHarness` exists for.
5. Name expansion and remap (`pattern.rs`, `ttf_builder/gsub.rs`) — the context-dependent parse
   rules are the trap.
6. Performance (see the rule above).
