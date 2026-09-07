# The Uniform editor

`uniform [DIR]` opens the editor on a font directory; with no argument it reopens the directory of
the last run. Every `.unf` in the directory is read together, exactly as `uniform build` reads it,
and the font, the diagnostics and the specimen are rebuilt in the background a moment after each
edit. The menus list every command with its shortcut; this page is about the behaviour a menu entry
cannot explain. `Ctrl` below is `Cmd` on macOS.

## Files

Files are shown in the sidebar; a file is opened by clicking it, created with Ctrl+N and renamed
with F2 over its entry. Opening a file canonicalizes it — spacing, quoting and comment placement are
rewritten to the serializer's form — so a hand-formatted file changes on its first save. Comments
survive.

**Saving** (Ctrl+S, Ctrl+Shift+S for all) happens off the UI thread, since the font directory is
often a network share where a write costs whole seconds. The file is marked clean at the revision
that was written, so typing during a save leaves it dirty and undoing back to what was written makes
it clean again. Quitting waits for pending writes, and a failed write cancels the quit.

**Changes made outside the editor** are watched for. A file you have not edited is reloaded in
place, as an undo entry, so Ctrl+Z still walks back to what was on screen. A file with unsaved edits
keeps them: it is flagged, a notice says so, and the next save asks before overwriting. Either is
postponed while the pointer rests on the surface the change would move (the sidebar for the file
list, the pane for its contents); a sticky notice says what is waiting, and clicking it applies the
change now. **F5** (*File ▸ Refresh filesystem*) asks for a check immediately, which is the way to
force one on a volume the operating system cannot watch.

*File ▸ Export* writes the built font to a file; the `build` subcommand is the same thing with more
options.

## Panes and navigation

The editor area splits into two panes side by side (Ctrl+Alt+←/→). A document is shown by at most
one pane — opening a file already on screen moves the focus there — and there is at most one empty
pane, so a split is offered only from a pane that has a document. Ctrl+Alt+H/L move the focus,
Ctrl+Alt+X swaps the panes, Ctrl+W closes the focused one, and dragging the divider onto either edge
closes the pane it collapses. Closing a pane leaves its document open, edits and undo history
included.

**Ctrl+click** on a name goes to its definition: a glyph, an anchor's other sign, a `remap` group's
`feature`, a name written as a pattern (the click lands on the block that declares it), a `$-N` or
`($N)` (on the group it names), or a glyph name mentioned in a `//` comment. **Ctrl+]** is the same
from the caret. A click on the declaration itself, or on a name nothing declares, opens the
**Search** pane listing every appearance — declarations first, then uses — so a typo in a `ref`
lists the lines that share it. Unopened files are searched from the directory snapshot, never from
disk. A `ref … goto` line redirects a jump to its wrapper glyph onward to the target, leaving two
history entries.

**Search.** The pane's header row is `[kind] [what to look for] n/m [Search]`, then how many files
the hits are spread over, and a message when the last thing asked for found nothing. Two kinds: **Text**, a verbatim substring — no
case folding and no collapsing of spaces, what is typed is what is looked for — and **Glyph**, every
appearance of a glyph name, matched by what a token denotes rather than by how it is written. Both
run over every file in the directory, open buffers as they stand and the rest from the snapshot, and
results come file by file in name order.

**Ctrl+F** and **Ctrl+Shift+F** reveal the pane with the box focused and its contents selected, so
typing replaces the last query; they pick the Text and Glyph kind respectively. Pressing either again — the box now holding the keyboard — steps the kind dropdown
forward or back instead, wrapping at both ends. Enter or the Search button runs it and puts the caret
on the first result; a glyph search lists the declaration first, so Ctrl+Shift+F, a name and Enter is
"go to that glyph". A search that finds nothing moves nothing and says so. **Ctrl+G / Ctrl+Shift+G**
step to the next and previous result, wrapping; **Esc** in the box hands the keyboard back to the
editor and does nothing else, so a Ctrl+G afterwards carries straight on. A search does not reach
inside a glyph's pixel rows: they are one grid line to the caret, not text.

**Ctrl+T / Ctrl+Shift+T** go back and forward through jumps. Going back restores the page that was
on screen, not merely the line.

**Folding.** A `glyph` block and a `#`/`##`/`###` heading section each fold to their first line;
Ctrl+; toggles the innermost group at the caret, and the gutter marker does the same. A glyph whose
grid draws taller than about two lines of text (a `scale N` glyph, typically) starts folded. The
minimap beside the editor shows `#` and `##` headings as readable text, so a file can be navigated
by section.

## Editing text

The text is edited with the usual keys; a `glyph` header and its pixel grid are **one block** to
Enter, to line-wise copy and cut with nothing selected, and to a paste onto a header. Undo is
per-file.

- **Ctrl+/** comments the selected lines out, or takes the comment off. A header and its grid
  travel together, and a commented grid comes back as a grid.
- **Ctrl+K** types a character by code point: a small field takes hex digits, shows the character
  as a preedit and names it in the status bar, and Enter commits it. It opens on the code point
  after the last one committed, or on the selected character if exactly one is selected.
- **F2** on a name renames the symbol across every file that mentions it, opening the unopened
  ones first. Which tokens are that kind of name follows the same classification the links use, so
  a `remap` group that reads like a glyph name is not touched.
- **Alt+wheel** or **Alt+↑/↓** step the number at the caret, whatever the pointer is over.
- **Completion** opens while a name is typed. Filtering stops at the name's last `:`, so the
  variants of a glyph are listed together; in an IDC slot the list is ordered by how well a variant
  fits the slot and drops one of the wrong size outright. On-demand shapes are never offered.
- A `map` or `assert shape` line shows the code points of literally written text beside it, and a
  dotted circle before a character the font gives no advance, so two spellings of one string can be
  told apart. Neither is part of the text.
- A `sample` line has a *Use* button that hands its text to the preview; a generated sample
  (`udhr-article1`, `subdivision-flags`) has none, since the editor has no data directory.

Editing a `glyph` header or a `ref` line reparses the file only once the caret leaves the line, so a
half-typed header does not demote its grid to text.

## Editing a glyph

A pixel grid is drawn on directly. **1** enters drawing mode on the grid at the caret; the palette
beside the grid shows the sub-pixel shapes, and the letter keys of its rows (`asdf`, `qwer`, `zxcv`)
cycle a family of shapes each. **2–9** select the glyph's layers — its `ref` and `anchor` lines, in
order — for moving with the mouse or the arrows. **Escape** returns to the text.

**`` ` ``** enters selection mode: a rectangle of pixels can be framed, moved, copied and pasted,
mirrored (M), flipped (I), rotated (J/K/L), inverted to the opposite shapes (O) or the opposite
bitmap spellings (Shift+O). With nothing framed the whole grid is the selection.

While a layer is selected, the glyphs that could attach at that anchor are drawn underneath the
grid (the **anchor shadow**), placed where they would land. A second **`` ` ``** inside selection
mode draws instead every glyph that refers to this one, each placed so that its copy of this glyph
lands on it (the **backreference shadow**) — the way to see whether the drawing still fits where it
is used. It is off by default, being costly on a widely used glyph.

**Resizing.** A glyph has two rectangles, dragged from two places:

- **F2** over a grid drags the *declared box* — what the glyph claims, and what every `ref` to it
  is measured against. The drawing does not move; the header's `origin`/`advance`/`extent` change,
  and every `ref` naming the glyph outright shifts to keep the drawing where it was. An
  anchor-placed `ref` is left alone, since it follows the anchors.
- With the backreference shadow up, the grid's own edge is grabbable and drags the *canvas* — how
  much room the drawing has. Growing it moves nothing that was drawn; the header gains an `origin`
  so the box stays put.

Arrow keys move an edge in either mode, Enter applies and Escape cancels. Sizes move in logical
pixels, so a `scale N` glyph moves by whole subcell blocks.

*View ▸ Show glyph metrics* draws each glyph's box, ascent and baseline over its grid.
*Inline once* replaces a `ref` with its target's own lines and *Inline to pixels* with the pixels it
draws; both are in the layer's menu (the thumbnail beside the grid, or a right-click on the grid
while that layer is selected) and both work on an IDC line too.

With `audit ref-image-path DIR` in the source, a strip of every chart's drawing of a code point is
shown above the first `glyph` line naming that code point (`han-4e00`, `ext-2a6d6-alt`). The
strips are cut from the published code charts by `scripts/extract_ref_charts.py`, whose docstring
says how; the row is one fixed height whatever the zoom, and drags sideways when wider than the
pane.

## The bottom panel

Ctrl+1/2/3 switch its tabs.

**Preview** shapes typed text with the built font through the platform's own stack — Core Text on
macOS, DirectWrite on Windows — and rustybuzz, which stands in for a browser; a combo picks the
backend and another forces the paragraph direction. The field is a small editor on the same keys as
the document, with a caret that follows bidirectional text: a plain arrow moves it visually, a
Shift+arrow extends the selection logically, and the caret leans the way its run reads. A character
the font cannot draw shows as `.notdef` rather than as a fallback font's glyph.

**Specimen** draws every mapped character and every glyph only a `remap` produces, one cell each,
grouped by block. Its options fill a block out to its whole range (with long blocks folded in the
middle, like the demo page), and a cell is tinted by the worst finding on the glyph it reaches — a
click on a tinted cell goes to the component the fault started at rather than to the character's
own glyph. Hovering a cell names the character with its `{gc=… ccc=… eaw=…}` properties.

**Issues** is the validation report with a filter per severity. Notes start hidden; right-click a
severity to show it alone. Chores — the clearance findings a build counts rather than prints — do
*not* start hidden: the filter here is live, so hiding a few thousand of them is one click, and it
is a click you make rather than one made for you.

The same findings also show up on the lines themselves: a line something was reported about is
tinted in that severity's color across the pane, and the message follows the text in the same
color, darker — a warning is a yellow line with a brown message on it. A line with more than one
finding shows the worst of them and counts the rest as `[+3]`. The filter above the list governs
this too, so hiding a severity there takes its tint off the text as well.

F6 runs the `assert` directives of the current file, Ctrl+F6 those of every file. F10/F11 step
through the faces the source declares; the chosen face is remembered. *Font ▸ Optimize clearance*
runs `uniform fix --optimize-clearance` against the open documents: the rewrites land in the
editor as one undo entry per file and nothing is written to disk until you save.

## View

Ctrl+= / Ctrl+- / Ctrl+0 zoom the editor; F12 toggles between the font being edited and the
system font for the editor's own text. *View ▸ Startup timing…* shows where the seconds before the
first frame went (the executable loading, the directory read file by file, the first font build),
and *View ▸ Rebuild timing…* what the last edit cost, on the background thread and on the UI
thread. Both are also printed to stderr with `UNIFORM_PERF` set; `uniform probe` is the windowless
form.

## What is remembered between runs

The font directory, the zoom, the panel sizes, the theme, the selected face per directory, the
bottom panel's tab, the issue filter, the specimen's options and the preview's text, size, backend
and direction. Not remembered: which files were open, the split, the caret, and the navigation history.
