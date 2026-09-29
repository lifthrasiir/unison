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

## How to draw a 1:1 slope

There are at least three possibilities when it comes to drawing a line with a slope of 1:1 (45 degrees) in a pixel grid:

```
A. /01/           B. /0@P           C. /0@@
   1/..              @P/.              @@0/
   +----+----+       +----+----+       +----+----+
   |  .:|:::'|       |  .:|::::|       |  .:|::::|
   |.:::|:'  |       |.:::|:::'|       |.:::|::::|
   +----+----+       +----+----+       +----+----+
   |:::'|    |       |::::|:'  |       |::::|:::'|
   |:'  |    |       |:::'|    |       |::::|:'  |
   +----+----+       +----+----+       +----+----+
```

Each possibility has a nominal (perpendicular) width of {1/2, 3/4, 1} * sqrt(2) ~= {0.707, 1.061, 1.414}. At first glance B looks optimal, as its nominal width is the closest to 1. It turns out that A is actually better however, for many reasons:

- A diagonal matches the visual weight of a straight line when its nominal width is slightly *less* than the straight line's, about 0.9 by the conventional type design. B already exceeds 1 before any rendering effect is considered, while A falls short and is then pushed towards the target by the next point. (A also uses exactly the same amount of ink per row as a straight line, but that is a coincidence rather than a rule: ink per row differs from the nominal width by a factor of 1 / max(|cos θ|, |sin θ|), which peaks at exactly 1:1 with sqrt(2) and is only about 12% at 2:1.)

- *Every* pixel constituting a line with a slope of 1:1 will be antialiased, while straight lines snap to the grid. Antialiased pixels do not really look like a "half" pixel at the target resolution: for dark text on a light background they look darker than their coverage, so a diagonal looks heavier than its nominal width suggests. A therefore gets closer to the target, while B and C look even heavier and thicker. The exact amount depends on how the renderer blends (with or without gamma correction) and on the text and background colors; for light text on a dark background the blending part partly reverses.

- A also fits better when the visual space is heavily limited.

The possibility A does have a small problem when it comes to the bitmap representation, as we have to choose between even and odd pixels for the bitmap. If this is not desirable (for example, if we want to keep the symmetry) then use the following variant of A:

```
A'. ./d/
    d//.
    +----+----+
    |    |.:::|
    |  .:|:::'|
    +----+----+
    |.:::|:'  |
    |:::'|    |
    +----+----+
```

A' is a bit more complicated but perfectly symmetric and has the same nominal and visual width as A.

## Han

Han characters need a separate mention because of its large number and complexity. Many characters (> 90%) can be composed from simpler components, but their size inventory varies and has a significant amount of local shaping rules. The current design tries to account for this observation.

All Han characters are named `han-XXXX` where `XXXX` is 4- or 5-digit hexadecimal Unicode code point. Regional variants are named `han-XXXX-R` where `R` is a region code (see `han.unf` for details) and numbered and named variants are named `han-XXXX.N` where `N` is either a hexadecimal selector number (VS17 = 0, VS18 = 1, ...) or a unique letter out of `xyzw...`. Those characters then have size and directional variants represented with Uniform's own variant label, such as `:15x16` or `:15x16.11x12` or `:9x16-l` and so on. Aliases are prevalent among them, for example regional variants are frequently also numbered.

Unison also tries to faithfully support major regional variants, though some of them might be artificial. For example, there are three possible variants for U+9751 靑 or U+9752 青 combined and their regional forms are fairly consistent. As such, if a certain character containing them is not attested in a particular region (say, Vietnam), it might still be rendered using the most consistent variant (`han-9752.0` in this case).

Uniform's IDC command support is heavily geared towards Han use cases and handles most common composition types. One-dimensional IDC recognizes an "across" and "cross" axis and semi-automatically chooses and locates components provided that their size in the cross axis equals that of the target glyph. Directional hints (e.g. `l` in `:9x16-l`) are also taken into account whenever appropriate. Two-dimensional enclosing IDC verifies the cavity size declared by components (e.g. `11x12` from `:15x16.11x12`) instead of axes. Some IDCs, notably an overlaying one, are not supported and have to be manually drawn.

Han glyphs are always designed to tightly contain grid pixels, so there must not be any margin around them. The single exception is `:15x16` which is a full character (there is a single column of implicit horizontal margin). Uniform provides a declaration of optional preferred margin for components and it should be used instead of manual margin instead. A more precise control is also available in the form of nested splits.

It is expected that a significant portion of Han characters is borderline impossible to represent using a 15x16 grid, in which case per-character scaling might be required. There is not any planned extension to allow fractional sizing yet. The current priority is to complete common characters and "easy" characters with a small number of components first.

### Regional components

Once a component gets regional variants, every character using it has to become regional as well. The `map` in `han.unf` prefers an unsuffixed `han-XXXX` over every regional one, so leaving an unsuffixed name around would silently override the region split; a regional component therefore has no unsuffixed name at all, and each of its users is rewritten into `glyph han-XXXX-($han-regions):15x16` with the component spelled `-($-1)`. A user that is already partially regional (`han-XXXX-(g|j|p)`) keeps `($-1)`, a user for a single region names that region, and a numbered variant names the numbered variant of the component that its reference shows. Numbered aliases of a converted user, previously `han-XXXX.0:* = han-XXXX:*`, should point to a concrete region instead (specifically, the first applicable region in the order of `ghtjkpv`).

The variant numbers and the region grouping of a character come from *its own* references, never from those of its component. For example, U+5206 分 has two forms because U+516B 八 has two (the right stroke is a plain ㇏ or starts with a short horizontal, 乁), but the two characters disagree on both counts: VS17 is the hooked 八 (`han-516b.0`) and the plain 分 (`han-5206.0`), and Vietnam uses the hooked 八 but the plain 分 (`han-516b-(j|k|p|v)` versus `han-5206-(g|h|t|v)`). The same holds one level up: U+7D1B 紛 groups its 分 as GHTJV/KP while its 糹 still follows `($-1)`, so the character is split into `han-7d1b-(g|h|t|j|v)` and `han-7d1b-(k|p)` with the 分 variant named explicitly in each.

Only a character whose references actually differ by region is made regional. Otherwise the component is named by the form that fits: `-R` when the character applies to a single locale and uses that locale's default form (U+7EB7 纷 uses `han-5206-g`, U+2B847 𫡇 uses `han-5206-v`), and `.N` when it either picks a variant other than that locale's default, or is shared by several locales that all use the same variant (`han-5206.0`). A single-region group can still be written as an alternation, `han-XXXX-(t)`, so that `($-1)` keeps working in its components.

Numbered variants are not limited to the forms of the component either. The IVD sometimes registers a form that the component no longer has, such as the 𠆢-topped 分 in U+5E09.1 帉 and U+9B75.2 魵; such a variant is drawn directly from the old parts (`ref han-201a2:9x7` plus `ref han-5200:5x9`) instead of adding a third variant to the component.

Ideally all component variants should share the same size and boundary, but this is not always possible. Examples include U+5DE8 巨 which has two variants one of which has two notches at the left, so a matching variant with the same visual width is one pixel narrower than that. In such cases variants have to be exhaustively described from users.

### Component forms

A component form that has no code point of its own (the upper 八 in 分, unlike U+201A2 𠆢 for 人) is typically a size variant of the character itself, with an optional directional hint for where it sits in the parent: `han-516b.0:15x6-u`, `han-516b.1:9x8-u` and so on. These are listed before the full `:15x16` drawings of the character.

A character built from such forms is written as an IDC rather than as `ref`s with hand-picked offsets, so that clearance is checked; a narrower lower part is centered with a nested split, and parts that interlock overlap with a negative gap:

```
glyph han-5206.0:9x16 9 16 // 分
⿱ han-516b.1:9x8-u -1 2|han-5200:5x9|2 // ⿱八刀
```

Note that nested splits could have been simplified if the `margin-x` flag were given to `han-5200:5x9` above.

Hardblanks tell the clearance check how far a neighbour may reach into the part. In 八 one row of the space between the two strokes is claimed where the strokes start to flare out, and everything below it is left open, so the top bar of 刀 may come up into the flare but not further; the cells outside the tops of the strokes are claimed as well.

### Checking the bitmap build

The outline and the bitmap build have to be checked separately, because a drawing that looks right in one can look wrong in the other. The published `.ttc` only has the outlines. To look at the bitmap, build a copy of `font/` with `meta bitmap-axis` added and instantiate the result at `BMAP=1`.

- Most shapes have a lit and an unlit spelling with identical geometry, so the lit cell of a diagonal is a free choice that leaves the outline alone. Use it to keep the staircase even: a column repeated in the middle of a diagonal (5, 4, 3, 3, 2) reads as a kink, while a repeat at the steep end (5, 4, 4, 3, 2) reads as a curve.
- A sheared on-demand stroke decides its bitmap by coverage, so a stroke that leans by less than a pixel over its height lights the same column all the way down. When a near-vertical stroke has to step in the bitmap, draw it with pixel pairs instead of a `ref`.
- On-demand glyphs have some specific pixel ratios that don't make a neat curve in the bitmap, like `5x2-ys1` (6 instead of 5 pixels lit). A common trick is to slightly adjust the dimension (e.g. `4p5r6x2-ys1` or `5p1r8x2-ys1`) to make it neat without visibly changing the shape.
- Keep the staircase consistent across the sizes of one component (e.g. 3, 3, 3, 3, 2, 2, 1, 0 at `:9x8` and 2, 2, 2, 2, 1, 1, 0 at `:7x7`), so the component looks the same in every character that uses it.
