//! `sample`: the ready-made specimen texts a source carries, grouped the way
//! they are offered.
//!
//! A `sample` line builds nothing. It is the one thing a source can say about
//! its own font that no font file has a field for — *this* is the text to read
//! it in — and the two places that answer that question read it here: the demo
//! page's sample panel ([`crate::render::demo`]) and the editor's preview.
//! Which is also why it is not a [`meta`](crate::meta) key: `meta` declares
//! what the font file carries, and a sample is carried by the page beside it.
//!
//! # Two levels, and why the label may carry a text itself
//!
//! `sample LABEL SUBLABEL` puts a text under a heading, so a family of texts —
//! a hundred and nineteen translations of one paragraph — is one entry on a
//! list rather than a hundred and nineteen. A label that stands for a single
//! text would then have to invent a sublabel for it, which is a second level
//! that says nothing; `sample LABEL` with no sublabel gives the *heading* the
//! text instead, and the page makes the heading itself the thing to click.
//!
//! A label may have one such line and no more, and no two lines of one label
//! may share a sublabel: the reader picks a text by its name, so two texts of
//! one name would be one entry the second of which is unreachable.
//! [`crate::issues`] reports both, and this collection keeps the first — a
//! source with a duplicate still shows what it can.
//!
//! # Modes, and why one is written rather than expanded
//!
//! The `: MODE` tail says how the `||` lines are *read*. Plain — no tail — is
//! the text itself, one line per line. `matrix` reads each line as an axis of
//! characters and offers their product: every character of the first line
//! against every character of the second, and so on, which is the specimen a
//! font's author actually wants for a pair of interacting characters (every
//! base against every mark, every jamo against every vowel) and which nobody
//! wants to type out by hand.
//!
//! The mode is kept beside the text and expanded by whoever shows it
//! ([`SampleText::expanded`]) rather than folded into the source's text at
//! collection time, because *not writing the product out* is the whole point:
//! four lines of eight characters are 32 characters written and 4096 shown,
//! and the demo page carries the four. Its `demo.js` expands the same way this
//! does, which is the one duplication the mode costs.
//!
//! # The modes that write their own text
//!
//! `udhr-article1` and `subdivision-flags` are the other kind: they take no
//! `||` lines at all, and stand for a body of text the *build* assembles from
//! its `-d` data directory — the translations of Article 1 a font can draw
//! whole, and the emoji tag sequence of every CLDR subdivision. Both were
//! already on `live.html`; writing them as a mode is what lets a source say
//! whether it wants them, since a page shows one because the font is about
//! those characters and not because the data file happens to be there.
//!
//! `udhr-article1` stands for a hundred-odd texts and not one, so it is only
//! ever written on a line with *no* sublabel: the sublabels are the
//! generator's — one per translation — and a line that named one would be
//! naming the group. `subdivision-flags` is a single text and takes either
//! form.
//!
//! Whoever holds the data directory fills these in ([`crate::render::demo`],
//! and `live.html`'s own sections). Everyone else — the editor, which has no
//! `-d` — sees the empty text they are written as, which is why a generated
//! sample has no *Use* button in the editor.
//!
//! # What the axes are
//!
//! The last line is the innermost axis and runs *along* a line; the one before
//! it runs down the lines; every earlier one is a block separated by one more
//! blank line than the axis inside it. So two lines are a table (rows from the
//! first, columns from the second), three are a page of such tables, and a
//! fourth is a run of pages — the rule stated once and read outwards. A line
//! with nothing on it is no axis: an empty axis has an empty product, and a
//! blank continuation is much more likely to be spacing than a claim that
//! there is nothing to show.

use crate::document::DocumentItem;

/// One heading's worth of texts.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleGroup {
    pub label: String,
    /// The text the `sample LABEL` line with no sublabel gave the heading.
    pub text: Option<SampleText>,
    pub items: Vec<SampleItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SampleItem {
    pub sublabel: String,
    pub text: SampleText,
}

/// How a `sample` line's `||` lines are read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SampleMode {
    /// The text as written.
    #[default]
    Plain,
    /// Each line an axis of characters; what is offered is their product.
    Matrix,
    /// Article 1 of the Universal Declaration of Human Rights, one text per
    /// translation the font can draw whole
    /// ([`crate::render::sample::udhr_selection`]).
    UdhrArticle1,
    /// One line per region of the emoji tag sequences its subdivisions have,
    /// read from the CLDR containment data.
    SubdivisionFlags,
}

impl SampleMode {
    /// The mode a line's `: MODE...` tail states.
    ///
    /// A word that names no mode is an error [`crate::issues`] reports by
    /// name; here it is simply not the mode, so a source with a typo in it
    /// still offers its text rather than offering nothing.
    pub fn from_tokens<S: AsRef<str>>(tokens: &[S]) -> SampleMode {
        for token in tokens {
            match token.as_ref() {
                "matrix" => return SampleMode::Matrix,
                "udhr-article1" => return SampleMode::UdhrArticle1,
                "subdivision-flags" => return SampleMode::SubdivisionFlags,
                _ => {}
            }
        }
        SampleMode::Plain
    }

    /// Whether the text is the build's to produce rather than the source's to
    /// write: a generated mode takes no `||` lines, and what it stands for is
    /// only there once a build has the data directory to read it from.
    pub fn is_generated(self) -> bool {
        matches!(
            self,
            SampleMode::UdhrArticle1 | SampleMode::SubdivisionFlags
        )
    }

    /// Whether the mode stands for a *list* of texts rather than one, which is
    /// why it can only be written on a line with no sublabel: the sublabels are
    /// the generator's to name.
    pub fn is_group(self) -> bool {
        matches!(self, SampleMode::UdhrArticle1)
    }
}

/// One offered text: what the source wrote, and how to read it.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleText {
    /// The `||` lines as written, joined by newlines.
    pub raw: String,
    pub mode: SampleMode,
}

impl SampleText {
    /// The text to actually show.
    ///
    /// The headless binary never asks: it hands the demo page the raw text and
    /// the mode, and the page is what expands one.
    #[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
    pub fn expanded(&self) -> String {
        match self.mode {
            SampleMode::Matrix => expand_matrix(&self.raw),
            // A generated mode has nothing written under it, so this is the
            // empty string: whoever has the data directory fills the text in
            // (`crate::render::demo`), and whoever does not — the editor's
            // *Use* button — offers nothing rather than offering a blank.
            _ => self.raw.clone(),
        }
    }
}

/// The product of `raw`'s lines read as axes of characters; see the module
/// docs for which axis runs where.
fn expand_matrix(raw: &str) -> String {
    let axes: Vec<Vec<char>> = raw
        .lines()
        .map(|line| line.chars().collect::<Vec<char>>())
        .filter(|axis| !axis.is_empty())
        .collect();
    let mut out = String::new();
    if !axes.is_empty() {
        push_matrix(&axes, &mut String::new(), &mut out);
    }
    out
}

/// Walk the axes outermost first, carrying the cell built so far.
///
/// The recursion is over the axes and not over the output, so the separator is
/// what the *remaining* depth says it is: the innermost axis writes cells side
/// by side, and every level outside it adds one more newline to what it writes
/// between its children.
fn push_matrix(axes: &[Vec<char>], cell: &mut String, out: &mut String) {
    let (axis, rest) = axes.split_first().expect("a non-empty axis list");
    let sep = "\n".repeat(rest.len());
    for (i, ch) in axis.iter().enumerate() {
        if i > 0 {
            out.push_str(&sep);
        }
        let len = cell.len();
        cell.push(*ch);
        if rest.is_empty() {
            out.push_str(cell);
        } else {
            push_matrix(rest, cell, out);
        }
        cell.truncate(len);
    }
}

/// The `sample` lines of a source, grouped by label in the order the labels are
/// first written, and each group's items in the order they are written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleSet {
    pub groups: Vec<SampleGroup>,
}

impl SampleSet {
    pub fn collect<'a>(items: impl IntoIterator<Item = &'a DocumentItem>) -> SampleSet {
        let mut set = SampleSet::default();
        for item in items {
            let DocumentItem::Sample {
                label,
                sublabel,
                mode,
                text,
                ..
            } = item
            else {
                continue;
            };
            let mode = SampleMode::from_tokens(mode);
            // A line with no `||` under it is an error `issues` reports; there
            // is no text to offer, so there is nothing to put on a list. A
            // generated mode is the one line that is *meant* to be empty here
            // — its text is filled in by the consumer that has the data.
            if text.is_empty() && !mode.is_generated() {
                continue;
            }
            let text = SampleText {
                raw: text.join("\n"),
                mode,
            };
            let group = match set.groups.iter().position(|g| g.label == *label) {
                Some(at) => &mut set.groups[at],
                None => {
                    set.groups.push(SampleGroup {
                        label: label.clone(),
                        text: None,
                        items: Vec::new(),
                    });
                    set.groups.last_mut().expect("just pushed")
                }
            };
            match sublabel {
                None => {
                    if group.text.is_none() {
                        group.text = Some(text);
                    }
                }
                Some(sublabel) => {
                    if !group.items.iter().any(|i| i.sublabel == *sublabel) {
                        group.items.push(SampleItem {
                            sublabel: sublabel.clone(),
                            text,
                        });
                    }
                }
            }
        }
        set
    }
}

/// The `: MODE` values a `sample` line may state; anything else is a mode
/// [`crate::issues`] rejects by name rather than one the build ignores.
pub const SAMPLE_MODES: &[&str] = &["matrix", "udhr-article1", "subdivision-flags"];

impl SampleSet {
    /// Whether any line of the source states this mode.
    ///
    /// The generated bodies of text are offered because a source asked for
    /// them and not because the build has a data file that would fit: see
    /// [`crate::render::sample`]'s `live.html` sections, which are the pages
    /// this answers for.
    pub fn uses(&self, mode: SampleMode) -> bool {
        self.groups.iter().any(|group| {
            group.text.as_ref().is_some_and(|t| t.mode == mode)
                || group.items.iter().any(|i| i.text.mode == mode)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::parse_document_from_str;

    fn set_of(src: &str) -> SampleSet {
        let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
        SampleSet::collect(&doc.items)
    }

    #[test]
    fn lines_sharing_a_label_are_one_group_in_written_order() {
        let set = set_of("sample Latin one\n|| A\nsample Greek x\n|| B\nsample Latin two\n|| C\n");
        assert_eq!(
            set.groups
                .iter()
                .map(|g| g.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Latin", "Greek"],
            "a group keeps the place its label was first written"
        );
        assert_eq!(
            set.groups[0]
                .items
                .iter()
                .map(|i| (i.sublabel.as_str(), i.text.raw.as_str()))
                .collect::<Vec<_>>(),
            vec![("one", "A"), ("two", "C")]
        );
    }

    #[test]
    fn a_line_with_no_sublabel_gives_the_heading_a_text() {
        let set = set_of("sample Pangram\n|| Sphinx\n|| of quartz\n");
        assert_eq!(
            set.groups[0].text.as_ref().map(|t| t.raw.as_str()),
            Some("Sphinx\nof quartz")
        );
        assert!(set.groups[0].items.is_empty());
    }

    /// A duplicate is an error `issues` reports; what is offered is the first
    /// of them, so the rest of the source still shows.
    #[test]
    fn a_duplicate_keeps_the_first() {
        let set = set_of(
            "sample L a\n|| first\nsample L a\n|| second\nsample L\n|| head\nsample L\n|| again\n",
        );
        assert_eq!(set.groups[0].items.len(), 1);
        assert_eq!(set.groups[0].items[0].text.raw, "first");
        assert_eq!(
            set.groups[0].text.as_ref().map(|t| t.raw.as_str()),
            Some("head")
        );
    }

    #[test]
    fn a_matrix_of_two_lines_is_rows_by_columns() {
        let set = set_of("sample M x : matrix\n|| abc\n|| DEF\n");
        assert_eq!(set.groups[0].items[0].text.mode, SampleMode::Matrix);
        assert_eq!(
            set.groups[0].items[0].text.expanded(),
            "aDaEaF\nbDbEbF\ncDcEcF"
        );
    }

    /// A third line is the innermost axis: the one that used to run along a
    /// line now runs down it, and the outermost is a block of its own.
    #[test]
    fn a_third_line_pushes_the_others_outwards() {
        let set = set_of("sample M x : matrix\n|| ab\n|| DEF\n|| 123\n");
        assert_eq!(
            set.groups[0].items[0].text.expanded(),
            "aD1aD2aD3\naE1aE2aE3\naF1aF2aF3\n\nbD1bD2bD3\nbE1bE2bE3\nbF1bF2bF3"
        );
    }

    /// Every further line is one more blank line between the blocks, which is
    /// the same rule the third one stated read outwards.
    #[test]
    fn a_fourth_line_is_one_more_blank_line() {
        let set = set_of("sample M x : matrix\n|| ab\n|| cd\n|| ef\n|| gh\n");
        let text = set.groups[0].items[0].text.expanded();
        assert!(
            text.starts_with("acegaceh\nacfgacfh\n\nadegadeh\nadfgadfh\n\n\nbceg"),
            "{text}"
        );
        assert_eq!(text.matches("\n\n\n").count(), 1, "{text}");
    }

    /// One line is the plain text it already reads as, and a line with nothing
    /// on it is no axis at all — an empty axis would empty the whole product.
    #[test]
    fn one_axis_is_the_line_itself_and_a_blank_line_is_none() {
        let set = set_of("sample M x : matrix\n|| abc\n||\n");
        assert_eq!(set.groups[0].items[0].text.expanded(), "abc");
    }

    /// The mode is the line's, so a heading that carries its own text carries
    /// its own mode with it.
    #[test]
    fn a_heading_text_carries_the_mode() {
        let set = set_of("sample M : matrix\n|| ab\n|| xy\n");
        let text = set.groups[0].text.as_ref().expect("heading text");
        assert_eq!(text.expanded(), "axay\nbxby");
    }

    /// A plain sample is untouched by any of this: what is written is what is
    /// offered, newlines and all.
    #[test]
    fn a_plain_sample_is_its_own_text() {
        let set = set_of("sample M x\n|| abc\n|| DEF\n");
        assert_eq!(set.groups[0].items[0].text.mode, SampleMode::Plain);
        assert_eq!(set.groups[0].items[0].text.expanded(), "abc\nDEF");
    }

    #[test]
    fn a_sample_with_no_text_is_not_offered() {
        assert!(set_of("sample Latin one\n").groups.is_empty());
    }

    /// A generated mode is the one line that is meant to carry no text: it is
    /// offered so that whoever holds the data directory can fill it in.
    #[test]
    fn a_generated_sample_with_no_text_is_still_offered() {
        let set = set_of("sample `UDHR Article 1` : udhr-article1\n");
        let text = set.groups[0].text.as_ref().expect("the heading's own text");
        assert_eq!(text.mode, SampleMode::UdhrArticle1);
        assert_eq!(text.expanded(), "", "there is nothing here to expand");
        assert!(set.uses(SampleMode::UdhrArticle1));
        assert!(!set.uses(SampleMode::SubdivisionFlags));
    }

    #[test]
    fn a_generated_mode_is_found_under_a_sublabel_too() {
        let set = set_of("sample Flags `All subdivisions` : subdivision-flags\n");
        assert_eq!(
            set.groups[0].items[0].text.mode,
            SampleMode::SubdivisionFlags
        );
        assert!(set.uses(SampleMode::SubdivisionFlags));
    }

    /// Only `udhr-article1` names its own sublabels; the other generated mode
    /// is one text and takes either form.
    #[test]
    fn only_the_list_writing_mode_is_a_group() {
        assert!(SampleMode::UdhrArticle1.is_group());
        assert!(!SampleMode::SubdivisionFlags.is_group());
        assert!(SampleMode::SubdivisionFlags.is_generated());
        assert!(!SampleMode::Matrix.is_generated());
    }
}
