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

use crate::document::DocumentItem;

/// One heading's worth of texts.
#[derive(Clone, Debug, PartialEq)]
pub struct SampleGroup {
    pub label: String,
    /// The text the `sample LABEL` line with no sublabel gave the heading.
    pub text: Option<String>,
    pub items: Vec<SampleItem>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SampleItem {
    pub sublabel: String,
    pub text: String,
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
                text,
                ..
            } = item
            else {
                continue;
            };
            // A line with no `||` under it is an error `issues` reports; there
            // is no text to offer, so there is nothing to put on a list.
            if text.is_empty() {
                continue;
            }
            let text = text.join("\n");
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

/// The `: MODE` values a `sample` line may state.
///
/// Empty: the tail parses so that the grammar is settled before there is
/// anything to put in it, and until there is, every mode written is one
/// [`crate::issues`] rejects by name rather than one the build ignores.
pub const SAMPLE_MODES: &[&str] = &[];

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
                .map(|i| (i.sublabel.as_str(), i.text.as_str()))
                .collect::<Vec<_>>(),
            vec![("one", "A"), ("two", "C")]
        );
    }

    #[test]
    fn a_line_with_no_sublabel_gives_the_heading_a_text() {
        let set = set_of("sample Pangram\n|| Sphinx\n|| of quartz\n");
        assert_eq!(set.groups[0].text.as_deref(), Some("Sphinx\nof quartz"));
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
        assert_eq!(set.groups[0].items[0].text, "first");
        assert_eq!(set.groups[0].text.as_deref(), Some("head"));
    }

    #[test]
    fn a_sample_with_no_text_is_not_offered() {
        assert!(set_of("sample Latin one\n").groups.is_empty());
    }
}
