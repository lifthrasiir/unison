# Design Notes for Unison

## Design Space

The basic working area is 8x16 pixels or 16x16 pixels (depending on the [East Asian Width](#east-asian-width)). This does *not* mean that the area is fully used! It is actually uncommon for a glyph to use the full area, and the design space is typically organized as follows:

- Upper margin: 3 pixels if non-CJK, none if CJK
- Lower margin: 3 pixels if non-CJK, 1 pixel if CJK
- Left margin: 1 pixel
- Right margin: none, effectively shared with the next glyph

As a result, most 8x16 glyphs have a design space of 7x10 pixels while most CJK glyphs have a design space of 15x15 pixels. The extra pixel is used for multiple purposes, including descenders, combining characters and more rarely excess pixels. For the last purpose it may be necessary to set the `advance` glyph flag.

Also being a pixel-based font, it is often necessary to center pixels. Since half-pixels are avoided as much as possible, we first try to align so that pixels are centered *within the design space*, and otherwise try to align so that pixels are centered *within the entire working area*. Since both regions differ by the odd number of pixels, we have to be centered in some way or another by then.

## Glyph Naming Convention

Glyph names are mostly derived from their Unicode character names, but significantly simplified and opinionated for the use in Unison. The following guideline applies:

- Most glyph names should start with a prefix denoting their primary category. In order to shorten names, language abbreviations (e.g. `gr-` for Greek) and country abbreviations (e.g. `ca-` for Unified Canadian Aboriginal Syllabics) are preferred over script names if the script is mainly used only for that language or region. Endonyms are discouraged (e.g. prefer `gr-` over ISO 639-1 `el-` for Greek) unless it's unambiguous and shorter in that way (e.g. `hy-` for Armenian). Latin and derivatives, common numerals and symbols are not prefixed due to their universal uses throughout the world.
- The first word apart from the prefix should constrain what the glyph can ever be: if the name starts with `gr-alpha` its base character should be GREEK SMALL/CAPITAL LETTER ALPHA for example. Conversely later words are increasingly more specific to preceding words.
- Leverage the existing naming convention if possible. For example many mathematical symbols are named after their LaTeX commands (e.g. `lfloor`, `sqcap`), while Kanas are based on Romaji inputs (e.g. `xtsu`).

In addition the following words are used to denote specific variants of a glyph. Quite a lot of them came from the Unicode's own conventions (but applied more consistently, e.g. U+00BF INVERTED QUESTION MARK is `ques-turned`). If multiple of them are applicable, they are concatenated and applied in this order unless there is a good reason to do otherwise.

| Word | Meaning |
|------|---------|
| `@-aux` | Typical names for auxiliary glyphs if no other appropriate names exist |
| `-half` | Glyphs for the `narrow` face, see Unison.unf for more information |
| `white` | Hollow shapes (unless emoji) |
| `black` | Solid shapes (unless emoji) |
| `upper` | Uppercase variant in bicameral scripts |
| `title` | Titlecase variant in bicameral scripts (rare) |
| `lower` | Lowercase variant in bicameral scripts |
| `reversed` | Horizontal mirroring |
| `inverted` | Vertical mirroring |
| `turned` | 180-degree rotation, preferred over `reversed-inverted` or (if both applicable) `inverted` |
| `rotated-cw` | 90-degree clockwise rotation |
| `rotated-ccw` | 90-degree counter-clockwise rotation |
| `u`, `d`, `l`, `r` | Directional variants (up, down, left, right), may be concatenated in this order |
| `horiz`, `vert` | Horizontal or vertical variants |
| `quadrant-<n>...` | Quadrants are 1-numbered from top to bottom and then from left to right (Z-order) |
| `A2B` | Preferred over `A-to-B` if A and B are alphabetic |
| `sm`, `sbg`, `bg` | Small, slightly big, big variants |
| `A-and-B` | Horizontal arrangement in the inline direction |
| `A-over-B` | Vertical arrangement, preferred over `B-under-A` etc. |
| `A-in-B` | General enclosure, preferred over `B-around-A` etc. |
| `comb`, `spacing` | Combining or spacing variant of diacritics |
| Numeric suffix or repeated letters | The number of occurrences of a certain feature or the glyph itself |

## Placeholder Glyphs

There are a number of placeholder glyphs in Unison which are subject to change. These are generally used to test complex shaping behaviors without "properly" drawing each constituent glyph. Such glyphs are visibly incomplete and mostly drawn in a hand-written style.

## East Asian Width

East Asian Width (EAW) is a Unicode property that classifies characters based on their expected display width in East Asian contexts. For the purpose of Unison, there are three main possibilities:

- **Narrow (Na)**, **Halfwidth (H)**: Consumes a single cell.
- **Wide (W)**, **Fullwidth (F)**: Consumes two cells.
- **Ambiguous (A)**, **Neutral (N)**: May consume either one or two cells, either depending on the context or because there is no known precedent.

Most importantly, most terminal emulators make use of EAW directly or indirectly (via `wcwidth`) to determine how many cells a character should consume. So any violation to EAW means a rendering error, even though the severity of that error varies: drawing Narrow characters in a wide cell is relatively harmless (though displeasing), but drawing Wide characters in a narrow cell can overflow into the next cell in some implementations.

After some headaches, we settled on the following rules (in the order of approximate priority):

1. Zero-width characters (gc=Mn/Me/Cf etc.) never consume any cells. Unlike other rules, this is a hard requirement and any (rare) exception should be documented here:
   - U+00AD: Soft hyphen should have a visible glyph to be used when it is actually made visible.
   - U+FFF9..FFFB: Interlinear annotation characters are typically made visible if not directly supported.
2. No grapheme clusters can exceed the sum of their constituent characters' inherent widths (e.g. derived from EAW). In addition, emoji grapheme clusters can consume at most two cells.
3. A single set of related characters (defined in proximity) should have the same width as long as possible.
4. Ambiguous/Neutral characters except for emojis should consume a single cell **in the Term face**. The Regular face is still free to draw them in two cells as appropriate.
5. If some characters can't be reasonably designed in a single cell, their support should be rescinded from the Term face.
6. Wide characters may be drawn in a single cell in order to satisfy the rule 3. Narrow characters shouldn't.
7. PUA characters are assumed to be assigned appropriate widths by their defining document (e.g. UCSUR) or context (e.g. logo).

Some consequences of these rules include:

- Non-CJK enclosed characters and arrows have a mix of Ambiguous and Wide characters. Per rules 4 and 6, they are consistently drawn in a single cell in the Term face, but in the Regular face they are drawn in two cells.
- Unified Canadian Aboriginal Syllabics are Neutral, but they can't be reasonably drawn in a single cell, so they are not supported in the Term face.
- Box-drawing characters are Ambiguous and drawn in a single cell in both faces for the consistency.
- Circles and squares are Ambiguous **except for a single Wide emoji for each set**. In this case we can't satisfy both rules 3 and 4 at the same time, so we chose to be consistent (because shapes are especially... shape-dependent) and always draw them without squashing. This is a rare case where the rule 4 is intentionally violated.
- Circles also pose an additional problem because they somehow include punctuations. Since we only explicitly avoid squashing, those characters are drawn in a single cell as long as the shape fits within the 8x16 grid. No squares are punctuations so they are not subject to this decision.
- Many PUA characters are wide even though they are all Ambiguous by the Unicode standard. We can assume they will get appropriate widths when they eventually get into the standard so it should be okay. Practically speaking it means they are not very usable in the terminal environments without an additional configuration, like monkey-patching `wcwidth`.
