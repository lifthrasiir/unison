# Design Notes for Unison

## Placeholder Glyphs

There are a number of placeholder glyphs in Unison which are subject to change. These are generally used to test complex shaping behaviors without "properly" drawing each constituent glyph. Such glyphs are visibly incomplete and mostly drawn in a hand-written style.

## East Asian Width

East Asian Width (EAW) is a Unicode property that classifies characters based on their expected display width in East Asian contexts. For the purpose of Unison, there are three main possibilities:

- **Narrow (Na)**, **Halfwidth (H)**: Consumes a single cell.
- **Wide (W)**, **Fullwidth (F)**: Consumes two cells.
- **Ambiguous (A)**, **Neutral (N)**: May consume either one or two cells, either depending on the context or because there is no known precedent.

Most importantly, most terminal emulators make use of EAW directly or indirectly (via `wcwidth`) to determine how many cells a character should consume. So any violation to EAW means a rendering error, even though the severity of that error varies: drawing Narrow characters in a wide cell is relatively harmless (though displeasing), but drawing Wide characters in a narrow cell can overflow into the next cell in some implementations.

After some headaches, we settled on the following rules (in the order of approximate priority):

1. Zero-width characters (gc=Mn/Me/Cf etc.) never consume any cells. Unlike other rules, this is a hard requirement.
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
