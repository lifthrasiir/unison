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

Subcommands (`migrate`, `build`) require native execution:

```sh
cargo run -r -- migrate -i ../unison/font/ -o font/    # legacy .txt → .unf
cargo run -r -- build -i font/ -o unison.ttf           # build TTF from .unf
```

## Editor Structure

- `UniformApp` (`app.rs`) — eframe entry point. Manages open documents, font build, derived data (resolved glyphs, issues).
- `OpenDocument` (`app.rs`) — one open file: `Document` + `Vec<DocLine>` (line-level model) + `EditorState`.
- `Sidebar` (`sidebar.rs`) — file list panel for the font directory. Lists `.unf` files; supports open, rename, create.
- `EditorState` (`editor/mod.rs`) — per-document editing state. `EditMode`: `Normal` (text editing), `GlyphEdit` (pixel painting), `LayerMove` (ref/layer repositioning).
- `preview/` — bottom panel for live font preview. HarfBuzz shaping + platform rasterizer (CoreText/DirectWrite).
- `specimen.rs` — specimen/sample rendering.
- `issues.rs` — cross-document validation (missing refs, duplicate maps, etc.).

## Document Format (.unf)

`migrated/*.unf` are the font data sources (one file per glyph category). Parsed/serialized in `document_io.rs`. Tokens use backtick-quoting: `` `foo bar` `` for tokens with spaces, four backticks (two for escape, two for quoting) for literal backtick.

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
