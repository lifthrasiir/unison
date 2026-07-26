# Uniform

Bitmap font editor with sub-pixel shape support and a TTF builder. egui/eframe GUI, Rust 2024 edition.

## Build

```sh
cargo build -r    # normal build
cargo test        # run tests
make              # build TTF/WOFF2 + samples
```

There are also Cargo aliases for cross-compiling WIndows executable. They should be used instead of normal commands when the current environment is *not* Windows:

```sh
cargo xb -r       # same as above but for Windows
cargo xrr         # run the compiled executable in Windows (only to be used by users)
```

Subcommands (`build`, `test`) require native execution:

```sh
cargo run -r -- build -i font/ -o unison.ttf           # build TTF from .unf
```

## Stack Overflow Monitor (`stackmon.rs`)

The main thread occasionally dies of a stack overflow with no reproducer. `stackmon` is the diagnostic for it; it is inert unless `UNIFORM_STACKMON=1`.

```sh
set UNIFORM_STACKMON=1
cargo xrr           # log goes to stderr and ./uniform-stackmon.log
```

On Windows a watchdog thread suspends the main thread every 250 ms and reads `Rsp`, so usage is measured across *all* code (egui, wgpu, DirectWrite) with no instrumentation. Past `UNIFORM_STACKMON_DUMP_PCT` (default 40%) of the stack it walks the suspended thread with `StackWalkEx` and logs a symbolized backtrace — i.e. the runaway call chain is captured *before* the process dies. A vectored exception handler also dumps `EXCEPTION_STACK_OVERFLOW` as a last resort (`SetThreadStackGuarantee` reserves 128 KiB so it can run). One backtrace is logged at startup as a self-test.

`crate::stackmon::phase("...")` markers in `app.rs::update` name the frame stage in each report. `crate::stackmon::probe()` records a high-water mark where sampling is unavailable (non-Windows). `[profile.release] debug = "line-tables-only"` exists so these backtraces have symbols; the `.pdb` must sit next to the `.exe`.

## Editor Structure

- `UniformApp` (`app.rs`) — eframe entry point. Manages open documents, font build, derived data (resolved glyphs, issues).
- `OpenDocument` (`app.rs`) — one open file: `Document` + `Vec<DocLine>` (line-level model) + `EditorState`.
- `Sidebar` (`sidebar.rs`) — file list panel for the font directory. Lists `.unf` files; supports open, rename, create.
- `EditorState` (`editor/mod.rs`) — per-document editing state. `EditMode`: `Normal` (text editing), `GlyphEdit` (pixel painting), `LayerMove` (ref/layer repositioning).
- `preview/` — bottom panel for live font preview. HarfBuzz shaping + platform rasterizer (CoreText/DirectWrite).
- `specimen.rs` — specimen/sample rendering.
- `issues.rs` — cross-document validation (missing refs, duplicate maps, etc.).

## Document Format (.unf)

`font/*.unf` are the font data sources (one file per glyph category). Parsed/serialized in `document_io.rs`. Tokens use backtick-quoting: `` `foo bar` `` for tokens with spaces, four backticks (two for escape, two for quoting) for literal backtick.

### Directives

- `font-meta height H ascent A descent D`
- `map CHAR = GLYPH` — cmap mapping
- `name-parts $NAME = token1 token2 ...` — name pattern definitions
- `remap FEATURE : [LOOKBEHIND... :] SOURCE -> TARGET [: LOOKAHEAD...]` — OpenType glyph substitution
- `feature NAME for SCRIPT... : REMAP_GROUP` — OpenType feature declaration with script filter
- `exclude-from-sample NAME`

### Glyph Blocks

`glyph NAME W H [flags...]` — glyph with pixel grid. Flags: `sticky`, `inline`, `advance N`, `left N`.

- Pixel rows follow immediately: 2 chars/pixel (`@@`=filled, `..`=empty, plus sub-pixel shape codes).
- `ref OTHER [COL ROW] [negated]` — composite reference. Omitting offset = auto-resolve from points.
- `point POS COL ROW` — anchor point for auto-ref alignment.
- `glyph NAME [flags...] = ALIAS` — simple alias (single ref, no grid).
- `glyph NAME [flags...]` — ref-only composite (no grid, followed by `ref`/`point` lines).
- NAME supports multi-glyph patterns: `(a|b*2|c*4)`, `($name-parts)`.

## Rendering Pipeline

`render/contour.rs`: pixel shape boundaries → contour tracing. `render/ttf_builder.rs`: contours → TrueType glyphs. `UNITS_PER_EM = 1024`.
