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
uniform build -i font/ -o unison.ttc     # build the typefaces
uniform test -i font/                    # run the `assert` directives
uniform font/                            # open the editor
```

Order matters in only two places: `color`, where a later definition wins over an earlier one, and
`face`, whose declaration order is the order the typefaces appear in the output. Everything else is
order-independent — `meta` in particular has no last-wins rule at all, because setting the same
thing twice is an error rather than an override. Glyph names must be unique across the whole
project; if two files define the same name, the first definition wins, the second is ignored, and
the report says so along with where the first one is.

Both `build` and `test` print every parse error, then a validation report of errors and warnings
with `file:line:` locations. Warnings do not stop the build, so a font that builds is not
necessarily a font without complaints — read the report.

A glyph only reaches the output font if something asks for it: a `map`, a `ref` from a glyph that
is itself reachable, a `remap` operand, or the `keep` flag. Unreachable glyphs are dropped and
reported as unused.

The one glyph nothing has to ask for is `.notdef`, which is what a renderer draws for a character
the font does not cover. TrueType reserves the font's first glyph slot for it, so a project that
defines `glyph .notdef` gets that drawing there — kept as if it said `keep`, since nothing in the
source is ever going to name it. A project that defines none gets a blank glyph in that slot.

### Typefaces and Slices

One project can describe more than one typeface. A **face** is one typeface in the output — a
standalone font file, or one font inside a collection. A **slice** is a named group of character
mappings, features and assertions that a face may include:

```
slice narrow
slice wide

face regular : wide
face term : narrow
```

Slices deliberately do not contain glyphs. Every face draws from the same glyph set; what differs
between two faces is which character reaches which glyph. That is what lets a collection store one
copy of the outlines and give each face its own `cmap`.

An unnameable **base slice** is in every face. Everything written without a qualifier belongs to it,
which is why a project with no `face` line at all still describes exactly one typeface and builds
the way it always did.

**Nothing overrides anything.** No face may include two slices that map the same character, and no
slice may re-state a mapping the base already has. Both are errors, and there is no precedence rule
to appeal to. The consequence is worth stating plainly, because it shapes how a split project is
written:

> A character whose mapping differs between faces must not be in the base slice at all.

So splitting a font by East Asian ambiguous width means *moving* those characters out of the base
into two slices, one per face — not adding an exception on top of a default. That is more work up
front and much less guessing later: which characters vary is visible in the source instead of being
the emergent result of a precedence rule.

`slice A = B C` is shorthand for "A also includes B and C", transitively. It is not a precedence
mechanism either; a conflict reached through it is the same error as any other, and a cycle is
reported whether or not any face uses it.

Faces appear in the output in declaration order. That is user-visible: a consumer that does not
choose a face gets the first one, and the sample and preview outputs show it too.

A face id ends up in a file name, so it is bounded more tightly than other names — see
[Identifier](#identifier).

### Output Files

The extension of an `--output` path picks the format, and whether the path contains a `%` picks
whether one file holds one face or all of them. `%%` is a literal `%`.

| Path | One face | Two or more |
| --- | --- | --- |
| `output.ttf` | the font | error: which one? |
| `output-%.ttf` | error: nothing to vary | one file per face |
| `output.ttc` | a one-face collection | the collection |
| `output-%.ttc` | error | error: a collection per face is not a thing |
| `output.woff2` | the font | error, see below |

Every combination is either meaningful or an error; nothing is silently reinterpreted, because the
failure mode of guessing here is a file that looks right and holds the wrong typeface. All of a
build's outputs are planned before any is written, so a wrong combination fails before it has
half-produced a set of files.

A collection stores each table once and points every face that uses it at the same offset. Since the
glyph store is shared, two faces of a width split differ only in `cmap` and `name`, and one `.ttc`
is about half the size of the same two faces written separately.

WOFF2 has no such option: a browser cannot select a face inside a collection, so a multi-face
`.woff2` is refused and the web form is one file per face.

```sh
uniform build -i font/ -o unison.ttc -o unison-%.woff2
```

(You can still make a WOFF2 version of the TTC file with the stock `woff2_compress` if you wish.
Uniform only prevents you from doing so without knowing what you are actually doing.)

### Glyph Metrics

Everything in a `.unf` file is measured in pixels, and the pixel grid is what defines the em. `meta`
gives the em height in pixels and how that height splits around the baseline:

```
meta height 16
meta ascent 14
meta descent 2
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
to be mapped or marked `keep` to survive.

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

### Identifier

A glyph name is made of letters, digits, `-`, `.`, `_` and `:`. The colon marks an
[alternative form](#alternative-glyphs); the rest is ordinary. The Unison's own convention is
`snake-case` but is by no means mandatory.

What the set leaves out is the point of it. Every character the [name pattern](#name-pattern) syntax
uses — `(`, `)`, `|`, `$`, `*`, `#` — is excluded, so a pattern that failed to expand cannot reach
the font as a name that merely looks odd. The rule is checked against *expanded* names, so how one
was written does not matter.

Slice ids use the same set without `:`. Face ids are narrower still, because `--output output-%.ttf`
puts one in a path: no `:`, no leading `.`, and never `.` or `..`. Two face ids that differ only in
case are rejected as well, since the file names they produce would collide on a case-insensitive
file system.

There is no `U+XXXX` glyph-name form. A range of hex-named glyphs is written `uni($#XXXX..YYYY)`,
which is partly what the inline hexadecimal range was added for. `U+XXXX` remains a *character*
spelling on the left of a `map`, which is a different context and unaffected.

### Auxiliary Glyph Name

A glyph name may start with `@`, which stands for the name of the last glyph declared *without* one.
It is a shorthand for naming a glyph's helpers after the glyph they belong to:

```
glyph foo         // the base
ref @-bar         // → foo-bar
glyph @-bar       // → foo-bar
ref @-baz         // → foo-baz
glyph @-baz       // → foo-baz
```

A header written with `@` is itself a helper and does **not** become the base, so a run of them all
hang off the same glyph rather than nesting: `@-baz` above is `foo-baz`, not `foo-bar-baz`. The
point is that a family of auxiliary glyphs can be named systematically without the base's name being
repeated on every line — and renaming the base renames the whole family with it.

The base is the declared name with its `:variant` suffix taken off, so a variant's helpers hang off
the glyph and not off the variant:

```
glyph foo:mono
ref @-bar:mono    // → foo-bar:mono
```

`@` is a name character in the first position only, and only in two places: a `glyph` header
(including both sides of an [alias](#glyph-glyph-alias)) and a `ref` target. Everywhere else — a
`map`, a `remap`, an `assert` — a glyph is named in full. A full name is of course still writable in
those two places as well; `@` is a shorthand, not a mode.

The substitution is textual and happens before anything else reads the name, so a base that is a
[name pattern](#name-pattern) carries through: under `glyph a($1..3)`, `ref @-b` is `a($1..3)-b`.
Written as `@`, a name is *stored* as `@`: the editor re-serializes files it opens, and what it
writes back is the `@` form, not what the `@` currently resolves to.

Above the first plain `glyph` header there is nothing for `@` to stand for. Such a name is left as
written and reported as an error rather than being resolved to a guess.

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
targets, `map` and `remap` operands, `assume unused`, and the character name of a `prop`.

| Form | Expands to |
| --- | --- |
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
* In a **`map` or `remap` operand**, an `assume unused` argument, or a **`prop` name**, the whole
  string behaves as if it were parenthesized, so `a*2|b` means `a`, `a`, `b`.
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
`glyph sp 8 16` is written.

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

The diamond and the four corner triangles tile the cell exactly, so the diamond can be enlarged by
two corners at a time. Two opposite corners give a thick diagonal stroke, two adjacent ones a
pentagon with one flat side; the complement of each is the pair of corners that was left out.

| Unlit | Lit | Shape | Complement |
| --- | --- | --- | --- |
| `//` | `d/` | diamond + the bottom-left and top-right corners (a thick ↗ stroke) | `'.` / `~_` |
| `\\` | `\b` | diamond + the top-left and bottom-right corners (a thick ↘ stroke) | `.'` / `_~` |
| `0D` | `1D` | diamond + the two left corners: flat left edge, apex at the right | `.>` / `.)` |
| `0v` | `1v` | diamond + the two top corners: flat top edge, apex at the bottom | `M0` / `M1` |
| `C0` | `C1` | diamond + the two right corners: flat right edge, apex at the left | `<.` / `(.` |
| `^0` | `^1` | diamond + the two bottom corners: flat bottom edge, apex at the top | `0W` / `1W` |

The remaining shapes are the halves of a 2:1 slope, for strokes that lean rather than run at 45°.
Each is a quarter-cell triangle with one leg along a full edge of the cell and its apex at the
midpoint of the opposite edge; the complement of each is the trapezoid that makes up the rest of
the cell.

| Unlit | Lit | Leg | Apex | Complement |
| --- | --- | --- | --- | --- |
| `v.` | `V.` | left edge | bottom center | `\v` / `\V` |
| `v'` | `V'` | left edge | top center | `/v` / `/V` |
| `` `v `` | `` `V `` | right edge | top center | `v\` / `V\` |
| `.v` | `.V` | right edge | bottom center | `v/` / `V/` |
| `\h` | `\H` | top edge | right center | `h~` / `H~` |
| `h/` | `H/` | top edge | left center | `~h` / `~H` |
| `h\` | `H\` | bottom edge | left center | `_h` / `_H` |
| `/h` | `/H` | bottom edge | right center | `h_` / `H_` |

A shape and its complement divide one cell exactly between them, which is what lets two strokes meet
inside a single cell without a seam or an overlap in the outline.

A row has to be exactly `W × 2` characters of valid codes. Once the first row has been recognized,
a later row of the wrong length, or one containing an unknown pair, is an error at that line. A
*first* row that does not parse is read as "this glyph has no rows", which usually surfaces a few
lines further down as unrecognized directives — check the declared width when that happens.

## Typeface Commands

### `face`: Typeface definition

```
face FACE [: SLICE...]
```

Declares one typeface in the output, including the named slices on top of the base slice every face
gets. A project that declares no face describes exactly one, with the base slice alone.

```
face regular : wide
face term : narrow
```

Faces are emitted in declaration order, so the first one is what a consumer that does not choose
gets, and what `--sample-html`, `--sample-png` and `--live-html` show.

Every face needs a name of its own: two faces producing the same family and subfamily would hide
each other in the system font list, and two sharing a PostScript name break PDF embedding. Both are
errors. Since a bare `meta family` reaches every face, the way to write this is one declaration per
face — see [`meta`](#meta-primary-font-metadata).

A face id becomes a file name through the `%` in an `--output` path, so it is bounded more tightly
than other names; see [Identifier](#identifier) and [Output Files](#output-files).

### `slice`: Slice definition

```
slice SLICE [= SUBSLICE...]
```

Declares a slice, optionally as the union of others. `map`, `feature` and `assert shape` may then be
qualified to it, and a `face` may include it.

```
slice narrow
slice wide
slice both = narrow wide
```

The `= ...` form is shorthand for "also includes these, transitively". It carries no precedence:
if a face ends up including two slices that map the same character, that is the same error whether
the two arrived directly or through a union. A cycle among slices is reported whether or not any
face reaches it, because it is a problem with the declarations rather than with any face.

A slice that nothing is qualified to gives every face including it nothing, and is reported as a
warning. Content counts transitively, so `both` above is not empty as long as `narrow` or `wide` is
not. The warning exists for the middle of a migration, where a mistyped slice name is otherwise
indistinguishable from a slice not yet filled in.

## Metadata Commands

### `meta`: Primary font metadata

```
meta [FACE :] KEY VALUE...
```

Sets one metadata value. Exactly one key per line: keys take anywhere from zero to ten values, so
two on a line could not be told apart without a separator.

An optional scope comes first. A bare key applies to every face, and `* : KEY` spells that out;
`FACE : KEY` applies to one face. The scope is recognized by the second token being a bare `:`,
which no key or value can be.

**Setting the same thing twice is an error**, even when the two values agree, and even through two
spellings — `family` and `name 1` are one slot. Scopes do not soften this: a bare key already
reaches every face, so stating a key bare *and* for a face gives that face two values, which is the
same conflict as a face including two slices that map one character. A value that varies per face is
stated once per face:

```
face regular : wide
face term : narrow

meta designer `Kang Seonghoon`
meta regular : family `Unison`
meta term : family `Unison Term`
```

#### Design metrics

These may only be set for every face. They fix how the pixel grid maps onto the em, and every face
draws from one glyph set, so they cannot differ between faces.

| Key | Meaning |
| --- | --- |
| `height N` | Em height in pixels. Default 16. |
| `ascent N` | Pixels above the baseline. Default 14. |
| `descent N` | Pixels below it. Default 2. |

`ascent + descent` should equal `height`; a mismatch is a warning, and a height of 0 is an error.
Keeping the em height a divisor of 1024 lets the builder emit exact hinting for the pixel size, so
16, 8 and 32 behave better than 20.

#### Values on the pixel grid

Declared in pixels and scaled by the same `1024 / height` as everything else, so they read in the
units the source is drawn in. All accept a negative number where the field allows one.

| Key | Meaning |
| --- | --- |
| `line-gap N` | Leading between lines. |
| `x-height N`, `cap-height N` | Defaults derive from the ascent. |
| `underline-at POS THICKNESS` | Position is relative to the baseline, so it is normally negative. |
| `strikeout-at SIZE POS` | Thickness and height above the baseline. |
| `subscript-at XSIZE YSIZE XOFF YOFF` | Size and offset of subscript text. |
| `superscript-at XSIZE YSIZE XOFF YOFF` | The same for superscripts. |
| `caret-offset N` | Horizontal shift of the text cursor. |

`caret-slope RISE RUN` is the one exception: it is a ratio, not a length, and is not scaled. `0 0` is
rejected.

#### Identity and classification

| Key | Meaning |
| --- | --- |
| `revision N` | Font revision as a decimal. Also the default for the version string. |
| `vendor-id TAG` | One to four printable ASCII characters. |
| `weight N` | 1 to 1000. Default 400. |
| `width N` | 1 to 9. Default 5. |
| `fs-type N` | Embedding permissions. |
| `panose A B C D E F G H I J` | Ten numbers. |
| `created DATE` | `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SSZ`. Fixed rather than the wall clock, so that building the same source twice produces the same bytes. |

A value outside a declared range is an error rather than something clamped, because a consumer
ignores it instead of correcting it.

#### Style flags

Written with no value at all.

| Flag | Effect |
| --- | --- |
| `bold`, `italic` | Both a style bit and a Mac style bit. |
| `oblique` | |
| `underscore`, `negative`, `outlined`, `strikeout` | Decorations. |
| `shadow` | Mac style only; there is no matching style bit. |
| `use-typo-metrics` | Prefer the typographic metrics over the Windows ones. |
| `fixed-pitch` | Claims every glyph has the same advance. |

The regular bit is set only when nothing else claims a style. It excludes exactly `bold`, `italic`
and `oblique` — not the decorations, which leave a font regular — and a font asserting both bold and
regular is a familiar and very visible bug.

`fixed-pitch` is a claim about the font, not something measured from it. Unison sets it despite
having half- and full-width cells, because terminal font pickers filter on it and every CJK
monospace font claims it for the same reason.

#### Name records

| Key | Name ID | Key | Name ID |
| --- | --- | --- | --- |
| `copyright` | 0 | `vendor-url` | 11 |
| `family` | 1 | `designer-url` | 12 |
| `subfamily` | 2 | `license` | 13 |
| `version-text` | 5 | `license-url` | 14 |
| `trademark` | 7 | `family-text` | 16 |
| `manufacturer` | 8 | `subfamily-text` | 17 |
| `designer` | 9 | `sample-text` | 19 |
| `description` | 10 | | |

`name ID VALUE` reaches any ID, including one with no keyword. A keyword and its numeric form are
the same slot.

Every name key takes an optional `@LANG` **BCP 47** tag before its value, which files the record
under that language:

```
meta family `Unison`
meta family @ko-KR `유니슨`
```

Only Windows-platform records are emitted, so `@LANG` is the only way to localize at all; the tag is
mapped to the language ID such a record is keyed by, and a tag with no mapping is an error rather
than a silently dropped record. A name asked for in a language that has none falls back to en-US, so
everything derived from the family has one definition even in a localized font.

#### Derived and computed

Name IDs 3 (unique ID), 4 (full name), 5 (version string) and 6 (PostScript name) are built from
family, subfamily and revision when they are not declared. Declaring one wins. They are emitted for
en-US only: a PostScript name is required to be English, and a localized one is not a thing. The
derived PostScript name is filtered to the restricted character set that format allows; it only has
to be valid, not a pretty transliteration of an exotic family name.

Coverage-derived fields are never declared, because they describe the font that came out rather than
an intent: the Unicode and code page ranges come from the `cmap`, the first and last character index
and the average character width from the glyphs, and the default and break characters are fixed.

### `prop`: Character properties the UCD does not have

```
prop CHAR [= NAME] [gc GC] [ccc N] [eaw EAW]
prop block NAME = U+XXXX[..YYYY]
```

States what a character *is*, for characters the Unicode Character Database says nothing useful
about — the Private Use areas, where a name and properties exist only because a font decided so.
The UCD gives every one of them the same answer (no name, `{gc=Co eaw=A}`), so without this the
editor and the sample can only show a bare codepoint:

```
prop block `Unison Symbols` = U+F0000..F00FF
prop U+F0000 = `UNISON LOGO` gc So eaw W
prop U+F0010..F001F = `UNISON BOX DRAWING-($#F0010..F001F)` gc So eaw W
prop U+F0020|U+F0021 = `UNISON (ALPHA|BETA)`
```

`CHAR` is the same character spelling `map` takes: one literal character, `U+XXXX`, an inclusive
`U+XXXX..YYYY` range, or a `|` list of those. `NAME` is a name pattern expanded against it in
lock-step, exactly as a `map`'s glyph name is — which is what states a whole range in one line.
`($#…)` names each codepoint of a range after itself; a list pairs one name to one character; a
single name over a range is that name for every character in it. The expanded name is then
upper-cased (ASCII only, so other scripts pass through as written), since `($#…)` produces the
lower-case hex a *glyph* name wants and a character name is upper case.

`gc` is a General_Category short name (`Lu`, `Lo`, `Mn`, `So`, …), `eaw` an East_Asian_Width short
name (`N`, `Na`, `A`, `W`, `F`, `H`), and `ccc` a canonical combining class number. A `gc` or `eaw`
outside those sets is an error: the line exists to be read, and a value nothing can be checked
against is worse than none. A `ccc` that is not a number in 0–255 makes the line malformed, as an
unknown keyword or a keyword without a value does; a malformed line is kept verbatim and reported as
an unrecognized directive rather than half-read.

Each property is independent, and **a line changes only what it states**. Where several lines cover
one character, each field comes from the last line that states it, so the properties of an area and
the name of one character in it are written separately:

```
prop U+F0000..F0FFF gc So eaw W        // the whole area, no names
prop U+F0000 = `UNISON LOGO`           // one name, properties untouched
```

Both parts are optional individually, but a line that states neither a name nor a property says
nothing and is rejected.

`prop block` records that an area of the codespace is claimed and for what. It is one more block of
the codespace, overriding whatever UCD block its area falls in — the UCD calls all of a Private Use
plane one block, so only the source can say what a font put there. The editor's specimen panel is
what reads it, to group its cells and head each group with a block name.

None of this reaches the font. The built TTF is byte-identical with or without `prop` — the
directive describes characters for the person reading the editor's status bar and the `sample.html`
tooltips, which is where a stated name and the `{gc=… ccc=… eaw=…}` group beside it come from.

### `exclude-from-sample`: Exclude a codepoint range from sample.html

```
exclude-from-sample U+XXXX[..YYYY]
```

Drops the given codepoints from the generated sample page. Several arguments may appear on one line,
each either a single character, a `U+NNNN` codepoint, or a range. This affects nothing but the
sample; the characters are still mapped in the font.

The editor's specimen panel reads the same set, but only while it is showing undeclared characters:
there it hides a whole displayed row whose every character is excluded, and keeps any row with one
character that is not. A run of hidden rows leaves an ellipsis row in its place.

Its use is bulk coverage that would swamp the page, such as most of the Hangul syllables:

```
exclude-from-sample U+AD00..D699
```

The sample page shows the first declared face, so this concerns that face alone. It is not slice-
qualified, and nothing about it reaches any output but the sample.

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
feature [SLICE :] NAME for SCRIPT[/LANGSYS]... : REMAP_GROUP
```

Unqualified, the feature belongs to the base slice and is therefore in every face. A `SLICE :`
qualifier restricts it to the faces that include that slice, which is how one typeface gets a
substitution another does not.

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
feature [SLICE :] NAME for SCRIPT[/LANGSYS]... : anchor ANCHOR_NAME
```

The `SLICE :` qualifier works exactly as it does on the substitution form.

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

* `keep` — put the glyph in the font whether or not anything asks for it, and do not warn about it
  being unused. A glyph that is mapped, named in a `remap` or used as a component is kept anyway, so
  this is for one that is reached by none of those and still has to exist. It is also the only way
  to declare a glyph with no body at all — see below.
* `inline` — never emit the glyph as a TrueType composite component; users of it get its contours
  copied in. Small shared fragments that are not glyphs in their own right (`dia-narrow`,
  `acute-left`) are declared this way.
* `mark` — the glyph is a combining mark: it goes into the GDEF mark class, and its `-name` anchors
  become mark attachment points.
* `desync` — the pixel grid is bitmap ink and nothing else: the outline build ignores its geometry
  and draws the glyph from its `ref` lines alone, while the bitmap build reads the grid as always.
  See [Grid-Only-For-Bitmap Glyphs](#grid-only-for-bitmap-glyphs).
* `advance N`, `left N`, `top N` — metrics overrides; see [Glyph Metrics](#glyph-metrics).
* `scale N` — the glyph's grid is N times finer in both directions. `W` and `H` stay in whole
  pixels, and the rows that follow are `W × N` cells wide and `H × N` tall. Use it when a shape
  needs detail below the pixel: `glyph flag-il-david 5 6 scale 2`. Refs into and out of a scaled
  glyph are rescaled automatically.

A glyph needs a pixel grid or at least one `ref` to exist at all. `advance`, `left`, `top` and
`anchor` do not make one buildable, and a glyph with none of the two never enters the font:
referring to it from a `map`, `ref` or `remap` is an error. For a deliberately blank glyph, use
`ref sp`, or declare dimensions and omit the rows.

`keep` is the exception. A glyph with the flag and no body at all is built as an empty outline that
carries nothing but its `anchor`s, and referring to it is fine:

```
glyph join-point keep
anchor +join 4 8
```

This is a placeholder: something for other glyphs to attach to, or a name to hold a slot in a
`remap`, without any drawing of its own.

A glyph whose name is a pattern is expanded into one glyph per name, in lock-step with the patterns
in its `ref` lines. Such a glyph cannot carry a pixel grid — a grid cannot be shared across
expansions — so it must be built from refs.

`NAME` may start with `@`, which stands for the last glyph declared without one — see
[Auxiliary Glyph Name](#auxiliary-glyph-name).

#### Grid-Only-For-Bitmap Glyphs

Ordinarily the two builds draw the same shapes and differ only in how finely: the grid *is* the
drawing, and the bitmap build squares off whatever the geometry says (see
[Pixel Grid](#pixel-grid)). The `desync` flag breaks that tie on purpose. The grid becomes bitmap
ink and nothing else: the outline build never reads its geometry and resolves the glyph from its
`ref` lines alone.

Paired with refs to `:zero` on-demand shapes — which are the mirror case, geometry that lights no
pixel (see [Bitmap Control](#bitmap-control)) — the two builds become independent drawings of one
glyph, each written where it belongs:

```
glyph tri desync 4 4
........
....@@@@
..@@@@@@
@@@@@@@@
ref 4x4-dr:zero 0 0
```

The outline build draws the exact triangle; the bitmap build draws the staircase written above, and
nothing rounds anything. That staircase is deliberately not one the rules of
[Bitmap Control](#bitmap-control) can produce: the default rule and `:ceil` both keep the lone apex
pixel (every cell the 45° hypotenuse cuts is covered exactly half, so the tie goes to lit), and
`:floor` drops the bottom-left step instead. Reach for `desync` when the small-size rendering you
want is not one the geometry rounds to.

The grid still declares the glyph's dimensions in both builds, so suppressing the outline never
changes an advance. `desync` on a glyph with no refs is legal and means exactly what it says: a
glyph with a bitmap and no outline.

### `glyph`: Glyph alias

```
glyph NAME [FLAGS...] = TARGET
```

Shorthand for a glyph consisting of one ref at offset (0, 0) with no flags of its own. Since the ref
carries no `inherit`, an alias exposes none of its target's anchors; write the block form with
`ref TARGET inherit` when it should.

Both `NAME` and `TARGET` may start with `@` (see
[Auxiliary Glyph Name](#auxiliary-glyph-name)); as with any header, an alias whose own name carries
one does not become the base.

### `ref`: Subglyph use

```
ref NAME [X Y] [FLAGS...]
```

Draws another glyph inside this one. `X Y` is the offset in cells of the *enclosing* glyph's grid,
with X rightward and Y downward. Omit it and the offset is derived from anchors (see
[Anchor Adjoinment](#anchor-adjoinment)); a negative offset is a bearing and is preserved as one.

The target may be a name pattern, and it may be an on-demand name that no `glyph` defines (see
[On-demand Glyphs](#on-demand-glyphs)). It may also start with `@`, the enclosing base glyph's name
(see [Auxiliary Glyph Name](#auxiliary-glyph-name)).

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
map [SLICE :] CHAR = GLYPH
```

Unqualified, the mapping belongs to the base slice and is in every face. A `SLICE :` qualifier puts
it in that slice instead, which is how two faces map one character to different glyphs:

```
map wide : ← = arrow-r2l
map narrow : ← = arrow-r2l-half
```

Since the base slice is in every face, a character mapped this way must not *also* be mapped in the
base — that would give a face two mappings for it, and there is no rule to choose between them. See
[Typefaces and Slices](#typefaces-and-slices).

The qualifier is told from the mapping by the second token being a bare `:`, which no character
spelling can be. `map : = colon` therefore still maps U+003A, and `map wide : : = colon` still
qualifies one.

Maps a character to a glyph in the font's `cmap`. `CHAR` is written as the character itself, as
`U+NNNN`, or as a `U+XXXX..YYYY` range; several characters may be listed with `|`. `GLYPH` is a name
pattern expanded in lock-step with the characters:

```
map U+0020 = sp
map ¨ = dia-above-spacing
map U+1F1E6..1F1FF = regional-indicator-($a-z)
```

Mapping the same codepoint twice is reported, as is mapping to a glyph that does not exist.

A variation selector cannot be mapped on its own. It reaches the font only as the second half of the
form below, whose glyph the build owns.

### `map BASE SELECTOR`: Map a variation sequence to a glyph

```
map [SLICE :] BASE SELECTOR = GLYPH
```

Maps a Unicode *variation sequence* — a base character followed by a variation selector — to a
glyph. The `SLICE :` qualifier works exactly as it does on the plain form.

```
map U+26AB U+FE0E = black-circle-6-inside
map U+0030 U+FE0F = num-zero-emoji
```

There are two spellings and each round-trips as it was written. `U+0030 U+FE0F` is two tokens; the
same pair pasted out of a character picker is *one* token holding two characters. Only that exact
shape — two characters, the second a selector and the first not — is read as a pair, so a pipe list
keeps its last alternative and a longer paste stays whole. The halves carry their spellings
independently, so `map 0 U+FE0F = x` is accepted too, and comes back as written.

Since a selector is invisible, the editor spells out the codepoints of a literally written sequence
beside it, on `map` lines and on `assert shape` alike.

The selectors are `U+FE00..FE0F`, `U+E0100..E01EF` and the Mongolian `U+180B..180D`/`U+180F`. That
is the set the *shaper* reads a variation sequence out of, which is the one that matters here: a
pair written outside it would never reach the lookup it was stated for.

#### Only a base and one selector

A longer sequence is an error, because the `cmap` format 14 subtable this compiles to holds a base
and one selector and nothing longer. Pasting an emoji keycap gives three characters:

```
map 0️⃣ = keycap-zero              // error: U+0030 U+FE0F U+20E3
```

The rest of the sequence belongs in a `remap`, against the glyph the pair produces:

```
map U+0030 U+FE0F = num-zero-emoji
remap keycaps : num-zero-emoji combining-keycap -> keycap-zero
```

#### Ranges

Either half may be a range or a `|` list, but not both — which one goes with which would have to be
either a zip or a cross product, and neither is more obviously right. `GLYPH` is expanded in
lock-step with whichever half varies:

```
map U+0030..0039 U+FE0F = num-($0..9)-emoji          // many bases, one selector
map U+4E00 U+E0100..E0102 = han-4e00-ivs($1..3)      // one base, many selectors
```

#### What it emits

Two things, from the one declaration, so that they cannot disagree:

* a **`cmap` format 14 entry**, which is how a conforming shaper reads a variation sequence — it
  resolves the pair before any `GSUB` lookup runs and removes the selector from the buffer;
* a **fallback `GSUB` ligature rule** — `base selector -> GLYPH` — in `ccmp`, at a lower lookup index
  than every rule the source wrote. On a shaper that honors `cmap` format 14 this never fires. It is
  there for one that does not: DirectWrite drops an unmatched selector before `GSUB` sees it, so a
  sequence written as a `remap` alone works in HarfBuzz and CoreText and silently fails on Windows.
  That is why this form exists rather than being spelled out by hand.

The rule is a ligature (two glyphs in, one out) and not a single substitution, because the selector
has to *leave* the buffer even when the target is the base's own glyph.

Which of format 14's two arrays a pair lands in is decided by the build, not stated: a pair whose
target is already what the base maps to goes in the Default UVS array, which carries no glyph id and
says only "this sequence is valid, use the base's glyph, and swallow the selector"; everything else
carries a glyph id in the Non-default array. A source that had to choose would only ever get it
wrong.

The build also synthesizes a blank, zero-advance glyph per selector used, and gives it an ordinary
`cmap` entry — the fallback rule's first element is found through a plain `cmap` lookup, so without
the entry it would never match. Those glyphs carry no name a source has to avoid: their internal
names use `@`, which is not in the [glyph-name character set](#identifier), and no name survives
into the font at all because `post` is version 3.0.

#### What is checked

* The base must be mapped, in a slice this pair can meet, or the fallback rule has no first glyph.
* The halves must be the right kind — a selector in the selector position, a non-selector in the
  base position. Both directions are checked, so a swapped line is caught rather than building a
  sequence nothing will ever match.
* Format 14 is keyed by codepoint but `GSUB` is keyed by glyph, so the two halves stop agreeing
  wherever two characters share a base glyph. Two pairs colliding on one base glyph and selector is
  an error; a pair whose base glyph another character also reaches is a warning, since the fallback
  rule will apply it to that character too and format 14 will not.

### `map generate`: Map characters to synthesized glyphs

```
map [SLICE :] generate CHAR [= GLYPH]
```

The `SLICE :` qualifier works exactly as it does on the plain form.

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
worth the ambiguity. It is also what tells `map generate Á = a-acute` from
`map U+0030 U+FE0F = num-zero-emoji`, which have the same shape otherwise.

`generate` never takes a [variation sequence](#map-base-selector-map-a-variation-sequence-to-a-glyph):
a variation sequence is its own canonical decomposition, so there is nothing to synthesize from. It
parses and is rejected by name rather than being read as something else.

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

A variation sequence cannot be written here. A selector has no glyph a source can name, because a
conforming shaper resolves the sequence and removes the selector before `GSUB` runs — a rule that
matched one would work on some shapers and not others. Use
[`map BASE SELECTOR`](#map-base-selector-map-a-variation-sequence-to-a-glyph), and write the `remap`
against the glyph it produces. Tag characters are the opposite case: they have no such path, so they
stay ordinary glyphs a rule names directly.

Lookbehind and lookahead turn the rule into a contextual one. They are matched but not replaced:

```
remap flag-tag : black-flag tag-($a-z|$0..9) -> black-flag-($a-z|$0..9)
remap flag-tag : black-flag-($a-z|$0..9) : tag-($a-z|$0..9) -> black-flag-($a-z|$0..9)-cont
```

Every operand is a name pattern, and all positions in one rule expand together, cycling
independently — the rule count is the least common multiple of the individual expansions.

#### A group is one lookup

**One group becomes one GSUB lookup, and a lookup is one left-to-right pass over the glyph
sequence.** The rules of a group are that lookup's *subtables*, in declaration order. This is the
single most consequential thing to know about writing them, because of what it implies at each
position of the pass:

* **The first matching rule wins.** The shaper walks the group's rules in order and stops at the
  first one that matches, then advances past the glyphs that rule consumed. No later rule of the
  same group gets a say at that position. So a longer rule has to be declared *before* a shorter one
  that is its prefix — `lt eq gt -> …` above `lt eq -> …`, or `<=>` shapes as `<=` followed by a
  bare `>`.
* **A rule never sees another rule's output at the same position.** Re-substituting something a
  group already produced takes a *second* group: passes are the unit of re-entry, subtables are not.
* **Lookbehind sees substituted glyphs; lookahead does not.** The pass has already rewritten what
  lies to the left and has not yet reached what lies to the right. A rule chaining off a lookbehind
  therefore repeats over a run of any length, while a rule chaining off a lookahead cannot: it has
  to spell its context out in the glyphs the pass started with, and its reach is bounded by how many
  rules are written. Turning the pass around with `reversed` swaps which side can chain, and
  `font/arrow.unf` uses both — a lookbehind chain for `<===` and a reversed one for `===>`.

Groups themselves are ordered by [`remap group`](#remap-group-group-declarations); every group is a
separate pass, and the passes run in that order.

### `remap group`: Group declarations

```
remap group NAME [reversed] [after GROUP]...
```

Declares a remap group and carries what belongs to the lookup as a whole rather than to any one
rule. It is optional: a group that is never declared is unreversed and unconstrained, and takes its
place in the order where its first rule appears.

A rule always writes a colon straight after its group name, and a declaration never does, which is
what tells the two apart — so a group named `group` needs no special spelling, and its rules read
`remap group : a -> b` as usual.

**Order.** Groups run in source order — file by file in filename order, and within a file in the
order the group is first seen — except where `after` moves one. `after` is repeatable and may name
any group; the result is a topological sort that keeps source order wherever the constraints leave
it free, so declaring `after` on one group never disturbs the rest. A cycle, or an `after` naming a
group that does not exist, is an error.

Where a group is attached with `feature` has nothing to do with when it runs. The two questions —
*when does this pass happen* and *what tag and script is it reachable under* — are answered in
different places on purpose.

**`reversed`** builds the group as a reverse chaining contextual single substitution: the pass runs
right to left instead of left to right. That inverts the rule about context above — a `reversed`
group's *lookahead* is matched against glyphs the pass has already rewritten, and so chains over a
run of any length, while its lookbehind cannot. OpenType allows only a single substitution to be
made this way, so every rule in a `reversed` group must be 1 → 1.

## Validation Commands

### `assert shape`: Shaping assertions

```
assert shape TEXT [@lang] [+feat|-feat...] [for SLICE...] : GLYPH [advance N] [offset X Y] : GLYPH ...
```

`for SLICE...` restricts the assertion to faces that include all of the named slices; without it the
assertion applies to every face. It is written `for ...` rather than as a leading qualifier because
the directive already uses `:` as a separator.

A combination no face satisfies is an error, not an assertion that quietly never runs. A test that
silently does not run is worse than one that fails.

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

Unlike `keep`, this does not keep the glyph in the font; it only says that its absence is
intentional. Use `keep` for a glyph that must be present, and `assume unused` for one that exists
for documentation, for a test, or as raw material for something else.

## On-demand Glyphs

Some shapes are not worth a `glyph` block. A `ref` (or a `map`, or a `remap` operand) may name a
glyph that nothing defines, and if the name describes one of the shapes below, it is synthesized on
the spot. Such a glyph is implicitly `inline`.

Every such name has the same skeleton — a **declared box**, an optional shape word, an optional
bitmap rule:

```
[-]W[pArR]x[-]H[pBrR] [ -ul | -ur | -dl | -dr | -circle | -polyN… ] [ :ceil | :floor | :zero ]
```

It is read strictly left to right and has to match in full: exactly one shape word, at most one
colon suffix, nothing else and nothing left over. `16x16-circle-ul` and `16x16-poly5-dr` name no
shape at all, and neither does a name carrying any other colon suffix — which is what keeps ordinary
glyph names with `-` or `:` in them, every alternative form included, falling through to the normal
lookup.

Whatever the shape, the synthesized glyph is `ceil(W) × ceil(H)` pixels: the box fixes the size, and
the shape only decides what part of it is inked.

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

### `-circle`: Inscribed circle and ellipse

`-circle` draws the circle inscribed in the declared box, sharing its center:

```
ref 16x16-circle
```

When the two sides differ, the shape is worked out in an auxiliary *square* box of side
`min(|W|, |H|)` — again sharing the center — and then carried onto the real box by the affine
transform that takes the auxiliary box to it. So `2x1-circle` is the ellipse that fills 2 × 1
exactly. A fractional or negatively signed dimension anchors the box exactly as it does for a
rectangle, so `-6p1r2x14-circle` is an ellipse flush against the right edge of a 7-cell grid.

A circle has no orientation, so `-cw`/`-ccw` do not attach to it.

### `-polyN[.MMM|rK][-cwR|-ccwR]`: Regular polygon and star

`-polyN` draws the regular N-gon inscribed in that same circle — outer points on the circle, center
shared, and by default one outer point at the top of the box:

```
ref 16x16-poly6          # a hexagon, point up
ref 16x16-poly3-cw180    # a triangle, point down
ref 16x16-poly5r2        # a pentagram
```

`.MMM` turns it into a concave 2N-gon by pulling the inner points towards the center. It is a
decimal fraction of one to three digits, `0.000` through `0.999`, measuring how far in from the
edges of the N-gon the inner points sit. `.000` — the default, and what you get by leaving it out —
leaves them on the edges, which is the plain N-gon; note that even then the inner points are nearer
the center than the outer ones, by a factor of `cos(pi/N)`. The closer to `1.0`, the sharper the
points.

`rK` instead picks the inner radius of the `{N/K}` star polygon, for `0 < K <= N/2`. K need not be
coprime with N (`poly6r2` is a valid hexagram) and is unrelated to any denominator in W or H.
`poly5r2` is the pentagram, near enough `poly5.528` to be indistinguishable but not equal to it: an
`rK` radius is irrational and no decimal spells it. `polyNr1` is the plain N-gon, and `rK` with
`2K = N` sends every inner point to the center, so the shape has no width and holds no ink.

`-cwR` and `-ccwR` turn the shape R degrees clockwise or counterclockwise about the shared center,
where R is below 360 and may carry up to three decimals (`-cw22.5`). The rotation happens in the
square auxiliary box, *before* the stretch onto a non-square declared box — so a rotated polygon in
a wide box is a stretched rotated polygon, not a rotated stretched one.

Several spellings mean the same shape, and they are treated as such: `poly6`, `poly6.000`,
`poly6r1`, `poly6-cw60` and `poly6-ccw60` all denote the plain hexagon, and a rotation is always
folded into the shape's own N-fold symmetry. (They remain distinct glyph *names*; it is the shape
that is shared.)

#### Bitmap Control

The font is built twice, and a synthesized shape has to decide which cells the bitmap build lights.
That decision is made per whole pixel, from the exact area the shape covers within it, and a suffix
on the name chooses the rule:

* Default — coverage of at least half the pixel lights it, ties included. The ½ tie is not
  hypothetical; it is every cell a 45° hypotenuse cuts.
* `:ceil` — any coverage at all lights the pixel.
* `:floor` — only a fully covered pixel is lit.
* `:zero` — nothing is ever lit: the shape exists for the outline build alone and contributes no
  bitmap ink. Together with a `desync` grid, which is the opposite (ink with no geometry), this is
  what lets one glyph carry two unrelated drawings; see
  [Grid-Only-For-Bitmap Glyphs](#grid-only-for-bitmap-glyphs).

None of these moves an outline. The geometry is identical in all four cases; only the bitmap flavor
of the font differs. Whole-pixel shapes cover every pixel completely and so come out the same under
every rule but `:zero`.

All four apply to every shape above, and they are where a circle or a polygon really needs them: a
curve grazes a great many pixels, and which of those the bitmap build lights is exactly this
choice.

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
