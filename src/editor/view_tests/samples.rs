//! `sample` lines in the editor: their `||` continuations are laid out with
//! them, and the *Use* button beside the header hands the text to the host.

use super::*;

fn sample_doc() -> String {
    String::from(
        "glyph a 2 2\n....\n....\nsample Latin `English pangram`\n\
         || The quick brown fox jumps over the lazy dog.\n\
         || Mr Jock, TV quiz PhD, bags few lynx.\n",
    )
}

/// DocLines: 0 the glyph header, 1 its grid (two rows in one line), 2 the
/// `sample` header, 3 and 4 its continuations.
///
/// A sample is one item spanning three lines, and every one of them is drawn:
/// nothing else in the format but a glyph block is more than a line, so the
/// layout had no reason to expect it.
#[test]
fn a_samples_continuations_are_laid_out_with_it() {
    let h = EditorHarness::new(&sample_doc());
    assert_eq!(shown_lines(&h), vec![0, 1, 2, 3, 4]);
    assert_view_consistent(&h);
}

/// The button is on the header and nowhere else: a continuation is a line of
/// the text, not a text of its own.
#[test]
fn only_the_header_carries_a_use_button() {
    let h = EditorHarness::new(&sample_doc());
    let lines: Vec<usize> = h.sample_use_buttons().iter().map(|(l, _)| *l).collect();
    assert_eq!(lines, vec![2]);
}

/// Pressing it hands the host the *dedented, joined* text — what the preview
/// is to show — and not the buffer's `||` lines.
#[test]
fn pressing_use_hands_over_the_whole_text() {
    let mut h = EditorHarness::new(&sample_doc());
    assert_eq!(
        h.click_sample_use(2).as_deref(),
        Some("The quick brown fox jumps over the lazy dog.\nMr Jock, TV quiz PhD, bags few lynx."),
    );
}

/// A `matrix` sample's text is its axes, and what *Use* is for is the product
/// of them: the preview would otherwise be handed the two lines an author
/// wrote instead of the specimen they wrote them for.
#[test]
fn pressing_use_expands_a_matrix() {
    let mut h = EditorHarness::new("sample Latin pairs : matrix\n|| ab\n|| xy\n");
    assert_eq!(h.click_sample_use(0).as_deref(), Some("axay\nbxby"));
}

/// The press is the button's: it must not also land the caret at the end of
/// the header, which is the line the button is painted past.
#[test]
fn pressing_use_does_not_move_the_caret() {
    let mut h = EditorHarness::new(&sample_doc());
    h.click_text(0, 0);
    let before = h.state.cursor;
    h.click_sample_use(2);
    assert_eq!(h.state.cursor, before);
}

/// A `sample` with nothing under it has no text to use, so there is nothing to
/// press either.
#[test]
fn a_textless_sample_gets_no_button() {
    let h = EditorHarness::new("sample Latin pangram\n");
    assert!(h.sample_use_buttons().is_empty());
}

/// A generated mode's text is the build's, assembled from the `-d` directory
/// the editor does not have. So it is written with no `||` lines and gets no
/// button: there is nothing here to hand the preview.
#[test]
fn a_generated_sample_gets_no_button() {
    let h = EditorHarness::new("sample `UDHR Article 1` : udhr-article1\n");
    assert!(h.sample_use_buttons().is_empty());
}
