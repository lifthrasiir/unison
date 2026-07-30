# Uniform File Format Reference

Uniform makes use of `.unf` source files to generate bitmap and outline typefaces.
The `.unf` file format is a structured text format that
allows for the definition of glyphs, metrics, and other font properties in a human-readable way.

TODO: This document is subject to change according to the development of the Unison font.

## Concepts

### Font Project

A font project is a directory of `.unf` files. Every file in it is read together and contributes to
one shared namespace, so a glyph defined in `latin.unf` can be referenced from `comb.unf` without
any import or include; splitting the sources by script or by Unicode block is a convenience for the
author, not a scoping mechanism. Files whose name begins with a dot are skipped, and so is anything
without the `.unf` extension.

```sh
uniform build -i font/ -o unison.ttf     # build the font
uniform test -i font/                    # run the `assert` directives
uniform font/                            # open the editor
```

Order matters in only two places: `font-meta` and `color`, where a later definition wins over an
earlier one. Everything else is order-independent. Glyph names must be unique across the whole
project; if two files define the same name, the first definition wins, the second is ignored, and
the report says so along with where the first one is.

Both `build` and `test` print every parse error, then a validation report of errors and warnings
with `file:line:` locations. Warnings do not stop the build, so a font that builds is not
necessarily a font without complaints — read the report.

A glyph only reaches the output font if something asks for it: a `map`, a `ref` from a glyph that
is itself reachable, a `remap` operand, or the `sticky` flag. Unreachable glyphs are dropped and
reported as unused.

### Glyph Metrics

Everything in a `.unf` file is measured in pixels, and the pixel grid is what defines the em. The
`font-meta` line gives the em height in pixels and how that height splits around the baseline:

```
font-meta height 16 ascent 14 descent 2
```

With this, the top edge of row 0 is 14 pixels above the baseline, the top edge of row 14 is the
baseline itself, and row 15 hangs below it. Rows grow downward, columns grow rightward, and the
glyph origin is the top-left corner of cell (0, 0). The builder scales the whole thing to 1024 units per em on the
way out; nothing in the sources is written in font units.

The advance width of a glyph defaults to the width of what it actually resolves to — the declared
`W` for a glyph with a grid, or the extent of the composite for a glyph made of `ref`s. Three flags
override the defaults:

| Flag | Effect |
| --- | --- |
| `advance N` | Advance width in pixels, independent of what is drawn. `advance 0` is what makes a combining mark zero-width. |
| `left N` | Shifts the outline N pixels to the right (negative moves left). The advance does not change. |
| `top N` | Shifts the outline N pixels down (negative moves up). The advance does not change. |

`left` and `top` shift the drawing relative to the origin, which is how a mark whose grid is drawn
in the top-left corner ends up positioned under the baseline:

```
glyph dia-below mark advance 0 left -3 top 14
ref dia-wide
anchor -below 2..3 0
anchor +below 2..3 2
```

A component's own `left`/`top` are compensated for when it is used as a `ref`, so the shift applies
once, where it was declared, and does not accumulate through composites.

Note that `left`/`top` are not the way to move a subglyph around inside a composite; a `ref` offset
does that. In particular a negative `ref` offset is a left side bearing and is meant to survive: the
origin stays at (0, 0), the outline keeps its negative coordinates, and the advance still measures
only what lies to the right of the origin.

### Anchor Adjoinment

An anchor is a named position, or a named range of cells, on a glyph:

```
anchor +above 4 1
anchor -center 4 7..8
```

A `+` anchor offers an attachment site. A `-` anchor asks for one. Inside a composite, each `ref`
with a `-name` anchor looks for a `+name` published by one of its siblings (or declared by the
composite itself) and is positioned so that the two coincide; the `+` is then consumed, and the
ref's own `+` anchors are published in turn for later refs. This is why most `ref` lines in the
sources carry no offset at all — the anchors place them.

Two anchors match only when their names agree **and their sizes agree**. A `-center` covering one
cell does not attach to a `+center` covering two, and the mismatch is reported rather than rounded
away. Ranges are how a glyph says how wide the site is: `anchor -center 4 7..8` wants a two-row
site at column 4.

Ambiguity is treated as an error, not as something to guess at:

* a `-` anchor with more than one size-matching `+` candidate attaches to nothing;
* if the set a composite would expose contains the same anchor name twice, neither copy is exposed.

In both cases the fix is to say what you meant explicitly — declare the anchor on the composite
itself, or give the ref an explicit offset.

Anchors also drive mark attachment in the built font, through `feature ... : anchor NAME`. A glyph
flagged `mark` contributes its declared `-name` anchor as the mark's attachment point; a base glyph
contributes its `+name`. Mark-to-mark stacking falls out of the same data: a mark that also declares
`+name` can itself be a base for the next mark.

### Alternative Glyphs

A colon in a glyph name makes it an alternative form of the name before the colon. `i-lower:dotless`
is an alternative of `i-lower`, `acute-above:wide` of `acute-above`. The relationship is by name
alone; there is no directive to declare it. Chained names work too, so `a:b:c` is an alternative of
both `a:b` and `a`.

Alternatives are chosen automatically during anchor adjoinment. When a ref's primary form has no
anchor that fits, its alternatives are tried, in alphabetical order, and the first one whose anchors
fit is substituted. Two situations trigger this:

* the primary cannot attach — no `+` of the right name and size is available for its `-` anchor, but
  an alternative's anchors match;
* the primary would attach fine but cannot *serve*: a later ref needs a `+name` that only the
  alternative publishes, or publishes it at the wrong size. This is how `i` loses its dot in front of
  a combining mark above. `i-lower` declares `+below` itself and keeps its dot under `dia-below`,
  but it does not declare `+above` — `i-lower:dotless` does, so a following `dia-above` pulls the
  dotless form in.

The same choice is made again in the built font, through a `ccmp` substitution generated for base
glyphs whose attachment anchor lives only on an alternative. The two paths are meant to agree: a
precomposed glyph and its decomposed input should shape to the same picture.

Alternatives are pulled into the font automatically when a feature needs them, so they do not have
to be mapped or marked `sticky` to survive.

### Anchor Inheritance

Attachment inside a composite happens regardless of flags. What a composite *offers to the outside*
is a separate question, and the answer is opt-in.

By default a composite exposes only the anchors it declares itself. The anchors of its components
stay private. A component's surviving anchors are forwarded only when its `ref` line says `inherit`:

```
glyph d-z-upper 16 16
ref d-upper 0 0
ref z-upper 8 0 inherit
```

A digraph takes marks over its second half, so that is the half whose anchors it passes on; the
first half keeps its own to itself.

Anything derived from the exposed set follows this rule — GPOS base anchors in the built font,
further composition in a parent glyph, and the anchor shadow in the editor. A circled letter
therefore offers nothing it did not say, which is usually what you want: the enclosing shape decides
where a mark goes, not the letter inside it.

There is one exception. Composites generated by `map generate` stand in for their own canonical
decomposition, so their synthesized refs carry `inherit` implicitly. If you replace one of those
with a hand-written `glyph` block, that decision comes back to you and has to be made per ref.

A simple alias (`glyph NAME = TARGET`) forwards nothing. To alias a glyph *and* its anchors, write
the block form:

```
glyph a-lower-alias
ref a-lower inherit
```

### OpenType Tags

Feature, script and language system tags are written as they appear in the OpenType registries, and
Uniform passes them through with only length and encoding checks. A tag longer than four ASCII
characters is an error rather than a silent truncation.

* [Feature Tags](https://learn.microsoft.com/en-us/typography/opentype/spec/featuretags)
* [Script Tags](https://learn.microsoft.com/en-us/typography/opentype/spec/scripttags)
* [Language System Tags](https://learn.microsoft.com/en-us/typography/opentype/spec/languagetags)

A `feature` target is either a script tag on its own (`latn`, `hang`, `DFLT`) or a script narrowed
to one language system below it (`latn/ROM`). The two forms are spelled out rather than told apart
by the tag itself, because the registries collide in exactly one place — `DFLT` is the default
*script*, `dflt` the default *language* — and because a language tag means nothing without its
script: `SRB` exists under both `latn` and `cyrl`.

Scope fallback in OpenType is replacement, not extension. A shaper reads `DFLT` only when the script
it wants has no record at all, and reads a language system's feature list *instead of* its script's
default list. The builder compensates by folding `DFLT`'s features into every declared script and
each script's defaults into every language system below it. Without that, adding one
`feature locl for latn/ROM` would cost all Latin text its `ccmp` and every mark attachment with it.

Directives that share both a tag and a target become a single feature record, with their lookups
accumulated in declaration order. The same tag under a different target stays separate.

## Lexical Structure

A `.unf` file is a sequence of lines. Each line is one directive, one glyph header, one `ref` or
`anchor` line belonging to the glyph header above it, one row of pixels, a comment, or blank.
Indentation is not significant, and no line continues onto the next.

### Quoting

Tokens are separated by whitespace. A token that has to contain a space, or that has to be empty, is
wrapped in backticks. An empty token is written as two backticks, and it is the usual way to put "no
name at all" into a list of alternatives:

```
name-parts $hangul-modern-init0 = `` $hangul-modern-init
```

Inside a quoted token, two consecutive backticks stand for one literal backtick; a single backtick
ends the quote, and the character after it must be whitespace or the end of the line. Outside
quotes, backticks are ordinary characters. A literal backtick as a whole token is therefore four
backticks: two to escape, two to quote.

The editor re-serializes files it opens, and quotes only what needs quoting, so hand-written quoting
of an ordinary name will not survive a save.

### Comments

`//` starts a comment that runs to the end of the line. It is recognized only at the start of a
token, so `` foo `//` bar // quux `` is three tokens and a comment.

Comments belong to the line they sit on and are preserved through editing, including on `ref` and
`anchor` lines and on a `glyph` header. A comment on its own line is kept as written, backticks and
all.

Pixel rows are the exception: `//` is a legal pair of pixel characters, and a row is never read as
having a comment.

### Name Pattern

A name pattern is a compact way to write a list of glyph names. It appears in glyph headers, `ref`
targets, `map` and `remap` operands, and `assume unused`.

| Form | Expands to |
| --- | --- |
| `U+XXXX..YYYY` | one `U+NNNN` name per codepoint in the range |
| `(a\|b\|c)` | the group spliced into the surrounding text: `foo-(a\|b)` → `foo-a`, `foo-b` |
| `a*N` | inside a group, that one alternative repeated N times |
| `(...**N)` | at the very end of a group, each alternative repeated N times |
| `$var` | the values of a `name-parts` definition |
| `$0..9`, `$#a0..af` | an inline numeric range, decimal or hexadecimal, zero-padded to the written width |

`$`-references are substituted textually before the pattern itself is parsed, so a reference is just
one alternative among the others and the forms mix freely:
`(foo|$bar|baz*5|$#00..ff*3**2)`.

When a pattern contains several groups, they cycle independently: the pattern's length is the least
common multiple of the group sizes, and group *k* contributes its `i % len(k)`-th alternative to the
*i*-th name. This is what makes a glyph block expand in lock-step with its refs, and a remap's source
in lock-step with its target:

```
remap regional-indicator : regional-indicator-($a-z) -> regional-indicator-($a-z)-left : regional-indicator-($a-z)
```

The same string can read differently depending on where it appears, so it is worth knowing the three
contexts:

* In a **glyph header**, a top-level `a|b` — one outside any parentheses — is a list of two verbatim
  names, taken as written and not expanded further. `a*2` there is a name containing an asterisk.
* In a **`map` or `remap` operand**, or an `assume unused` argument, the whole string behaves as if
  it were parenthesized, so `a*2|b` means `a`, `a`, `b`.
* In a **`ref` target** inside a pattern glyph block, only parenthesized groups and a bare `foo*N`
  repeat are recognized; a top-level `a|b` with no group and no repeat stays one literal name.

Writing the parentheses removes the question, and is what the sources do.

Expansion is capped at 65536 names.

### Pixel Grid

A glyph declared with dimensions is followed by exactly that many rows of pixels, two characters per
pixel. `..` is an empty cell and `@@` is a full one:

```
glyph a-upper 8 16
................
................
................
....../1@@1\....
..../1@@P0@@1\..
../1@@0/..\0@@1\
..@@@@......@@@@
..@@@@......@@@@
..@@@@@@@@@@@@@@
..@@@@......@@@@
..@@@@......@@@@
..@@@@......@@@@
..@@@@......@@@@
................
................
................
```

If every cell is empty the rows may be omitted entirely, which is how a blank glyph like
`glyph sp 8 16 sticky` is written.

Every other character pair names a *sub-pixel shape*: a piece of the cell, described exactly, that
the outline build traces as geometry. Each shape has two spellings, differing in whether the cell is
also lit in the bitmap build. This is the whole trick behind the hybrid design, and it is the thing
to get right when drawing:

* the **outline build** draws the shape's geometry, whichever spelling you used;
* the **bitmap build** throws the geometry away and draws a full square for every lit cell, nothing
  for the rest.

So a diagonal stroke crossing two cells is written as two triangles, one lit and one not, and comes
out as an antialiased diagonal at large sizes and as a staircase of whole pixels at small ones.
Compare `/1@@0/` in the sources: a lower-right half that lights its cell, a full cell, and an
upper-left half that does not.

The catalog, unlit spelling first:

| Unlit | Lit | Shape |
| --- | --- | --- |
| `..` | `__` | empty cell |
| `88` | `@@` | whole cell |
| `0\` | `1\` | lower-left half, cut by the ↘ diagonal |
| `\0` | `\1` | upper-right half, same diagonal |
| `0/` | `1/` | upper-left half, cut by the ↗ diagonal |
| `/0` | `/1` | lower-right half, same diagonal |
| `0>` | `1>` | left quarter wedge, apex at the cell center |
| `0P` | `1P` | top wedge |
| `<0` | `<1` | right wedge |
| `d0` | `d1` | bottom wedge |
| `>0` | `>1` | everything but the left wedge |
| `0d` | `1d` | everything but the top wedge |
| `0<` | `1<` | everything but the right wedge |
| `P0` | `P1` | everything but the bottom wedge |
| `<>` | `{}` | diamond on the four edge midpoints |
| `><` | `)(` | left and right wedges together |
| `0B` | `1B` | top and bottom wedges together |
| `2>` | `3>` | triangle on the left edge, apex at the middle of the right edge |
| `2P` | `3P` | same, based on the top edge |
| `<2` | `<3` | same, based on the right edge |
| `d2` | `d3` | same, based on the bottom edge |
| `>2` | `>3` | everything but the left-based triangle |
| `2d` | `3d` | everything but the top-based one |
| `2<` | `3<` | everything but the right-based one |
| `P2` | `P3` | everything but the bottom-based one |
| `\.` | `b.` | quarter-size corner triangle, bottom left |
| `.\` | `.9` | corner triangle, top right |
| `/.` | `P.` | corner triangle, top left |
| `./` | `.d` | corner triangle, bottom right |
| `\@` | `9@` | everything but the bottom-left corner triangle |
| `@\` | `@b` | everything but the top-right one |
| `/@` | `d@` | everything but the top-left one |
| `@/` | `@P` | everything but the bottom-right one |

The remaining shapes are the halves of a 2:1 slope, for strokes that lean rather than run at 45°.
Each is a quarter-cell triangle with one leg along a full edge of the cell and its apex at the
midpoint of the opposite edge:

| Unlit | Lit | Leg | Apex |
| --- | --- | --- | --- |
| `v.` | `V.` | left edge | bottom center |
| `v'` | `V'` | left edge | top center |
| `` `v `` | `` `V `` | right edge | top center |
| `.v` | `.V` | right edge | bottom center |
| `\h` | `\H` | top edge | right center |
| `h/` | `H/` | top edge | left center |
| `h\` | `H\` | bottom edge | left center |
| `/h` | `/H` | bottom edge | right center |

and their complements — the trapezoids that make up the rest of the cell:

| Unlit | Lit | Complement of |
| --- | --- | --- |
| `v\` | `V\` | `` `v `` |
| `\v` | `\V` | `v.` |
| `v/` | `V/` | `.v` |
| `/v` | `/V` | `v'` |
| `h~` | `H~` | `\h` |
| `_h` | `_H` | `h\` |
| `h_` | `H_` | `/h` |
| `~h` | `~H` | `h/` |

A shape and its complement divide one cell exactly between them, which is what lets two strokes meet
inside a single cell without a seam or an overlap in the outline.

A row has to be exactly `W × 2` characters of valid codes. Once the first row has been recognized,
a later row of the wrong length, or one containing an unknown pair, is an error at that line. A
*first* row that does not parse is read as "this glyph has no rows", which usually surfaces a few
lines further down as unrecognized directives — check the declared width when that happens.

## Metadata Commands

### `font-meta`: Primary font metadata

```
font-meta height H ascent A descent D
```

Sets the em height in pixels and its split around the baseline. The three keywords may be given in
any order and in separate `font-meta` lines; the last value for each wins. Defaults are height 16,
ascent 14, descent 2.

`ascent + descent` should equal `height`; a mismatch is a warning, and a height of 0 is an error.
Keeping the em height a divisor of 1024 lets the builder emit exact hinting for the pixel size, so
16, 8 and 32 behave better than 20.

### `exclude-from-sample`: Exclude a codepoint range from sample.html

```
exclude-from-sample U+XXXX[..YYYY]
```

Drops the given codepoints from the generated sample page. Several arguments may appear on one line,
each either a single character, a `U+NNNN` codepoint, or a range. This affects nothing but the
sample; the characters are still mapped in the font.

Its use is bulk coverage that would swamp the page, such as most of the Hangul syllables:

```
exclude-from-sample U+AD00..D699
```

## Non-Glyph Definition Commands

### `color`: Color definition

```
color NAME = #RRGGBB[AA] [coloronly|monoonly]
```

Names a color for use in `ref ... fill`. The value is a hex literal or the name of a color defined
earlier; alpha defaults to opaque.

The optional visibility keyword is inherited by every ref that fills with this color, which saves
repeating it. `coloronly` means the layer exists in the color build (`COLR`/`CPAL`) but not in the
monochrome outlines; `monoonly` is the reverse. A white card face is a good example of the first —
invisible on paper, needed to cover the black beneath it in color:

```
color card-border = #404040
color card-white = #ffffff coloronly
color card-black = #000000
color card-red = #ff0000
```

### `name-parts`: Name parts definition

```
name-parts $NAME = TOKENS...
```

Defines a reusable list of alternatives for `$NAME`, substituted into any name pattern that mentions
it:

```
name-parts $a-z = a b c d e f g h i j k l m n o p q r s t u v w x y z
```

The `$` is part of the name. Names may contain letters, digits, `-` and `_`. An undefined reference
is left in place verbatim rather than expanding to nothing, so a typo shows up as a missing glyph
whose name still contains a `$`.

### `feature`: OpenType feature definition

```
feature NAME for SCRIPT[/LANGSYS]... : REMAP_GROUP
```

Attaches every `remap` in the named group to an OpenType feature, under one or more targets:

```
feature ccmp for DFLT : flag
feature ljmo for hang : hangul-ljmo
feature locl for latn/ROM latn/MOL : romanian-comma
```

The group name is arbitrary and links the two directives; it has no meaning in the output font. See
[OpenType Tags](#opentype-tags) for how targets are written and how scope fallback is handled.

### `feature ... anchor`: Anchor definition for OpenType feature

```
feature NAME for SCRIPT[/LANGSYS]... : anchor ANCHOR_NAME
```

Declares that the named anchor participates in mark attachment under this feature, which is what
turns `+above`/`-above` pairs into a GPOS mark-to-base (and mark-to-mark) subtable:

```
feature ccmp for DFLT : anchor below
feature ccmp for DFLT : anchor above
feature ccmp for kana : anchor kana-mod
```

Each declared anchor name becomes one mark class. Anchors that no `feature` line mentions still work
for composition inside the sources; they just do not reach the font as attachment points.

Alternative glyphs needed to make an attachment work — a dotless `i`, a narrower diaeresis — are
pulled into the font here, along with the `ccmp` substitutions that select them.

## Glyph Definition Commands

### `glyph`: Glyph definition

```
glyph NAME [W H] [FLAGS...]
[PIXEL_GRID]
```

Defines a glyph. With `W H`, the header is followed by `H` rows of pixels, `W` cells wide (see
[Pixel Grid](#pixel-grid)). Without them, the glyph is built from the `ref` lines that follow.
Either way, `ref` and `anchor` lines may follow the header, and a glyph may have both a grid and
refs — the grid is drawn at offset (0, 0) and the refs on top of it.

Flags may appear in any order, before or after the dimensions:

* `sticky` — keep the glyph in the font even though nothing maps or references it, and do not warn
  about it being unused. Placeholder and spacing glyphs need this.
* `inline` — never emit the glyph as a TrueType composite component; users of it get its contours
  copied in. Small shared fragments that are not glyphs in their own right (`dia-narrow`,
  `acute-left`) are declared this way.
* `mark` — the glyph is a combining mark: it goes into the GDEF mark class, and its `-name` anchors
  become mark attachment points.
* `advance N`, `left N`, `top N` — metrics overrides; see [Glyph Metrics](#glyph-metrics).
* `scale N` — the glyph's grid is N times finer in both directions. `W` and `H` stay in whole
  pixels, and the rows that follow are `W × N` cells wide and `H × N` tall. Use it when a shape
  needs detail below the pixel: `glyph flag-il-david 5 6 scale 2`. Refs into and out of a scaled
  glyph are rescaled automatically.

A glyph needs a pixel grid or at least one `ref` to exist at all. `advance`, `left`, `top` and
`anchor` do not make one buildable, and a glyph with none of the two never enters the font:
referring to it from a `map`, `ref` or `remap` is an error. For a deliberately blank glyph, use
`ref sp`, or declare dimensions and omit the rows.

A glyph whose name is a pattern is expanded into one glyph per name, in lock-step with the patterns
in its `ref` lines. Such a glyph cannot carry a pixel grid — a grid cannot be shared across
expansions — so it must be built from refs.

### `glyph`: Glyph alias

```
glyph NAME [FLAGS...] = TARGET
```

Shorthand for a glyph consisting of one ref at offset (0, 0) with no flags of its own. Since the ref
carries no `inherit`, an alias exposes none of its target's anchors; write the block form with
`ref TARGET inherit` when it should.

### `ref`: Subglyph use

```
ref NAME [X Y] [FLAGS...]
```

Draws another glyph inside this one. `X Y` is the offset in cells of the *enclosing* glyph's grid,
with X rightward and Y downward. Omit it and the offset is derived from anchors (see
[Anchor Adjoinment](#anchor-adjoinment)); a negative offset is a bearing and is preserved as one.

The target may be a name pattern, and it may be an on-demand name that no `glyph` defines (see
[On-demand Glyphs](#on-demand-glyphs)).

Flags:

* `negated` — subtract the subglyph's shape from what is drawn so far instead of adding it. Useful
  for punching a hole through a shape rather than drawing around it.
* `inherit` — expose this ref's surviving anchors as the composite's own. See
  [Anchor Inheritance](#anchor-inheritance).
* `fill COLOR`, `fill fg` — draw this layer in a color, for the `COLR`/`CPAL` build. `COLOR` is a
  `#RRGGBB[AA]` literal or a name from a `color` directive; `fg` means the text color, whatever the
  client is painting with.
* `coloronly`, `monoonly` — restrict the layer to the color build or to the monochrome one. A `fill`
  by color name inherits the visibility of that color, so the keyword is only needed to override it.

Layers are drawn in the order written.

### `anchor`: Anchor definition

```
anchor +NAME X[..X2] Y[..Y2]
anchor -NAME X[..X2] Y[..Y2]
```

Declares an attachment site (`+`) or an attachment request (`-`) at a cell or rectangle of cells in
this glyph's grid. Both coordinates accept a `start..end` range, inclusive, with the end no smaller
than the start.

The size of the range is part of the anchor's identity: attachment requires the same name *and* the
same width and height, which is how a mark that needs a two-row site is kept away from a base that
offers a one-row one. When both sizes exist, they are usually two alternatives of the same mark
(`dia-above` and `dia-above:narrow`), and the right one is chosen for you.

Anchors declared on a glyph are always its own; anchors arriving through a ref are exposed only with
`inherit`.

## Character Mapping Commands

### `map`: Map characters to glyphs

```
map CHAR = GLYPH
```

Maps a character to a glyph in the font's `cmap`. `CHAR` is written as the character itself, as
`U+NNNN`, or as a `U+XXXX..YYYY` range; several characters may be listed with `|`. `GLYPH` is a name
pattern expanded in lock-step with the characters:

```
map U+0020 = sp
map ¨ = dia-above-spacing
map U+1F1E6..1F1FF = regional-indicator-($a-z)
```

Mapping the same codepoint twice is reported, as is mapping to a glyph that does not exist.

### `map generate`: Map characters to synthesized glyphs

```
map generate CHAR [= GLYPH]
```

Builds a composite from the character's Unicode canonical decomposition and maps the character to
it. Each component of the decomposition must itself be mapped somewhere in the project; the
generated glyph refs those glyphs, in NFD order, with no explicit offsets, so anchors do the
positioning:

```
map generate Ӓ
```

The composite is named `uniXXXX` unless `GLYPH` names it, and `GLYPH` is expanded in lock-step with
`CHAR` exactly as a plain `map` target is. A character with no canonical decomposition is an error —
use a plain `map` for it — and so is one whose decomposition includes an unmapped codepoint.

The synthesized refs carry `inherit`, because the composite stands in for its decomposition and has
to keep offering what the components offered. Rewriting one by hand as a `glyph` block plus a `map`
means deciding, per ref, whether to keep that.

The `generate` keyword is not optional; a bare `map CHAR` reads too much like the plain form to be
worth the ambiguity.

### `remap`: Substitution rules

```
remap FEATURE : [LOOKBEHIND... :] SOURCE... -> TARGET... [: LOOKAHEAD...]
```

Declares a GSUB substitution belonging to a group, which a `feature` directive then attaches to an
OpenType feature. Source and target are both *lists* of glyph names, and the two lengths pick the
lookup type:

| Source → Target | Lookup |
| --- | --- |
| 1 → 1 | single substitution |
| 1 → N | multiple substitution |
| 1 → 0 (empty target) | removal |
| N → 1 | ligature |

N → M and N → 0 have no OpenType lookup type and are rejected rather than approximated.

Lookbehind and lookahead turn the rule into a contextual one. They are matched but not replaced:

```
remap flag-tag : black-flag tag-($a-z|$0..9) -> black-flag-($a-z|$0..9)
remap flag-tag : black-flag-($a-z|$0..9) : tag-($a-z|$0..9) -> black-flag-($a-z|$0..9)-cont
```

Every operand is a name pattern, and all positions in one rule expand together, cycling
independently — the rule count is the least common multiple of the individual expansions.

Rules in the same group become lookups in the order they are declared.

## Validation Commands

### `assert shape`: Shaping assertions

```
assert shape TEXT [@lang] [+feat|-feat...] : GLYPH [advance N] [offset X Y] : GLYPH ...
```

Shapes `TEXT` against the built font and compares the result glyph by glyph. This is the font's own
regression suite; `uniform test -i font/` (or `make test`) runs every one of them and exits nonzero
on a failure.

Each `:`-separated segment describes one output glyph: its name, and optionally the advance it
should have (in font units) and the offset it should be placed at (in pixels, X rightward and Y
downward). Omit them to check the glyph name alone.

```
assert shape ï : uni00EF
assert shape i̤ : i-lower : dia-below offset -4 0
assert shape j̈ : j-lower:dotless : dia-above:narrow offset -5 3
```

`+feat` and `-feat` turn a feature on or off for the run. `@lang` is a **BCP 47** language tag
(`@ro`), deliberately not the `script/LANG` language system that a `feature` line declares: an
assertion states what a real client hands the shaper, and deriving the OpenType language system from
it is the shaper's job — part of what is being tested. Writing `@ROM` on both sides would make the
two agree by construction and stop the assertion from ever noticing that Romanian text never reaches
the declared tag.

Text containing more than one script is split into runs and shaped run by run, the way a browser
would.

### `assert same` / `assert distinct`: Glyph equality assertions

```
assert same NAME...
assert distinct NAME...
```

Compares fully resolved glyphs — geometry, not names — and reports when two that should be identical
are not, or when two that should differ have collapsed into the same picture. Both take two or more
names.

`assert same` pins a deliberate identity, usually one that composition is supposed to produce:

```
assert same quadrant-12 upper-4-over-8
```

`assert distinct` is the error-proofing tool: it fails the moment two characters that are supposed to
be visually separable start rendering the same, which is easy to do by accident when a shape is
adjusted.

### `assume unused`: Suppress unused-glyph warnings

```
assume unused NAME...
```

Silences the unused-glyph warning for the named glyphs, which accept name patterns:

```
assume unused placeholder-(8|16)
```

Unlike `sticky`, this does not keep the glyph in the font; it only says that its absence is
intentional. Use `sticky` for a glyph that must be present, and `assume unused` for one that exists
for documentation, for a test, or as raw material for something else.

## On-demand Glyphs

Some shapes are not worth a `glyph` block. A `ref` (or a `map`, or a `remap` operand) may name a
glyph that nothing defines, and if the name describes a rectangle or a triangle, it is synthesized
on the spot. Such a glyph is implicitly `inline`.

The only colon suffixes recognized here are `:ceil`, `:floor` and `:zero`. A name carrying any other
one is not an on-demand name at all, so ordinary glyph names with colons in them — every alternative
form — fall through to the normal lookup.

### `[-]W[pArR]x[-]H[pBrR][-ul/ur/dl/dr]`: Sized rectangle and triangle

The simplest form is `WxH`, a filled rectangle W cells wide and H cells tall, both nonzero:

```
ref 22x14 1 1 fill #E1E1E1
```

A dimension may also be fractional, written `A[pBrR]` for `A + B/R`. So `1p2r3` is 1⅔ and `6p1r2` is
6½; `1p2r3x4` is a rectangle 1⅔ × 4. The denominator must be at least 2, the numerator smaller than
it, and when both dimensions are fractional they must share the same denominator. A whole part of
zero is allowed as long as the fraction is not.

A fractional shape does not fill its last cell, so it has to say which end of that cell the leftover
goes to. By default the ink starts at the left (or top) edge and the gap falls at the far end; a
leading `-` flips it, pushing the ink flush against the right (or bottom) edge. The two halves of a
tricolour need one of each:

```
ref 6p1r2x14 2 1 fill #10069f
ref -6p1r2x14 15 1 fill #d50032
```

The plain integer form fills its cells exactly, has no such choice, and rejects the minus sign.

Adding `-ul`, `-ur`, `-dl` or `-dr` makes the shape a right triangle with legs of the given size,
the right angle at the named corner of the bounding rectangle (upper left, upper right, down left,
down right).

#### Bitmap Control

The font is built twice, and a synthesized shape has to decide which cells the bitmap build lights.
That decision is made per whole pixel, from the exact area the shape covers within it, and a suffix
on the name chooses the rule:

* Default — coverage of at least half the pixel lights it, ties included. The ½ tie is not
  hypothetical; it is every cell a 45° hypotenuse cuts.
* `:ceil` — any coverage at all lights the pixel.
* `:floor` — only a fully covered pixel is lit.
* `:zero` — nothing is ever lit: the shape exists for the outline build alone and contributes no
  bitmap ink.

None of these moves an outline. The geometry is identical in all four cases; only the bitmap flavor
of the font differs. Whole-pixel shapes cover every pixel completely and so come out the same under
every rule but `:zero`.

### Colored Variant Synthesis (`:mono`, `:color`)

When a name `X` is undefined but both `X:mono` and `X:color` exist, `X` is synthesized as a glyph
that picks between them by rendering mode: `X:color` in the color build, `X:mono` in the monochrome
one. Referring to `X` then works in both, without the referring glyph knowing which build it is in.

This is how a flag can carry a full-color rendering and a legible monochrome fallback under one
name, with everything that refers to the flag written once:

```
glyph flag-gbeng:color 24 16
...
glyph flag-gbeng:mono
ref black-flag-wide
ref sm-g-upper 5 2 negated
...
```

Note that `:mono` and `:color` are alternatives in the ordinary sense as well, so both remain
addressable on their own.
