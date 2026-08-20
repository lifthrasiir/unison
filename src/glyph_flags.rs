//! Which glyphs the diagnostics report is about, glyph by glyph.
//!
//! The report [`crate::issues`] produces is a flat list of findings located by
//! `file:line:`, which answers "what is wrong with the source" but not "is
//! *this* glyph one of the broken ones" — and the specimen grid, which draws a
//! cell per character, asks the second question a few thousand times a frame.
//! [`collect`] turns the first into the second: one [`GlyphFlag`] per glyph
//! name, absent for a glyph nothing was said about.
//!
//! # Two states, not four
//!
//! Only [`Severity::Error`] and [`Severity::Warning`] flag anything, and an
//! error on a glyph hides a warning on the same glyph — a cell has one
//! background, and the worse finding is the one worth painting.
//! [`Severity::Todo`] and [`Severity::Note`] are deliberately not flags: a Todo
//! is a normal state of the source (a Han glyph on the work queue, tens of
//! thousands of them at once), so painting it would tint
//! nearly the whole grid, and a Note asks for no action at all.
//!
//! # Attribution: the line, or the one glyph that named itself
//!
//! A finding is normally located by *item*, and an item can be a pattern — the
//! Han sources are one `glyph han-($#4e00..9fff)` line standing for eighteen
//! thousand glyphs. Faulting a line therefore faults every glyph it stands for,
//! which is right when the fault really is in what is written (two IDC lines,
//! `advance` and `extent` both stated, a glyph nothing uses) and catastrophic
//! when it is not: one stray warning would tint the whole URO block.
//!
//! So a finding may name the glyph it is about, and [`Issue::glyph`] wins over
//! the line when it does. Only two paths can disagree between the expansions of
//! one line, and both fill it in: the expansions of a pattern share a body and
//! differ only in the names substituted into it, so what varies is whether a
//! substituted `ref` target **exists**
//! ([`crate::render::ttf_builder::expand`]) and, through that, whether an
//! **anchor** derives against it ([`crate::issues`]). An IDC line is the third:
//! its components expand with the block too, and the split is then solved from
//! the boxes *that* glyph's parts declare, so every finding it makes names the
//! glyph it made it about.
//!
//! A finding with no glyph of its own is matched back to its item by line
//! rather than by carrying an [`ItemRef`] on every [`Issue`], because an
//! `Issue` is produced in forty-odd places and only this one consumer wants the
//! provenance. `item_line_starts` is sorted, so the item a line belongs to is
//! one binary search.
//!
//! # Where the fault is, as opposed to where it shows
//!
//! A flag carries the glyph it *started* at, which is not the glyph the flag is
//! on once it has travelled. That is the difference between the cell to look at
//! and the line to fix: `map U+4EC2 = han-4ec2` reaches a glyph declared by a
//! pattern covering a whole block, and its fault is really in the
//! `han-4ec2:15x16` it refs. So the specimen tints the cell for `han-4ec2` and
//! sends a click to `han-4ec2:15x16`. For a glyph faulted directly — every
//! source that is not built out of parts — the two are the same name and
//! nothing changes.
//!
//! # Propagation
//!
//! A glyph built out of a broken glyph is broken too — a Han composite whose
//! component is missing draws nothing, and the cell to look at is the
//! composite's, since that is the one a character maps to. So the flags are
//! pushed backwards along the `ref` edges of the *expanded* items, which is
//! where an `⿰⿱⿲⿳` line has already become the refs it stands for
//! ([`crate::render::ttf_builder::expand`]). A cycle terminates because a flag
//! is only ever pushed on when it *raises* the target, and there are two
//! levels to raise through.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::document::{Document, DocumentItem, GlyphName};
use crate::issues::{Issue, Severity};
use crate::render::ttf_builder::Expansion;
use crate::resolve::ItemRef;

/// What the report says about one glyph. Ordered worst-last, so combining two
/// findings on one glyph is `max`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GlyphFlag {
    Warning,
    Error,
}

/// The glyphs [`Severity::Warning`] or worse was said about, directly or
/// through something they are built out of. A glyph nothing was said about is
/// simply absent — the map is expected to be small next to the glyph set.
#[derive(Clone, Debug, Default)]
pub struct GlyphFlags {
    /// Glyph → its flag and the glyph the flag started at (itself, for one
    /// faulted directly).
    flags: HashMap<String, (GlyphFlag, String)>,
}

impl GlyphFlags {
    #[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
    pub fn get(&self, glyph: &str) -> Option<GlyphFlag> {
        self.flags.get(glyph).map(|(f, _)| *f)
    }

    /// The glyph whose own line the flag came from — `glyph` itself unless it
    /// inherited the fault through a `ref`. See the module docs.
    #[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
    pub fn source(&self, glyph: &str) -> Option<&str> {
        self.flags.get(glyph).map(|(_, src)| src.as_str())
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.flags.is_empty()
    }

    /// Raise `glyph` to at least `flag`, blaming `from`, and report whether
    /// that changed anything — which is what keeps the propagation below
    /// finite. A flag that does not raise leaves the blame alone too, so the
    /// worst fault names its own source and ties go to the first arrival.
    fn raise(&mut self, glyph: &str, flag: GlyphFlag, from: &str) -> bool {
        match self.flags.get_mut(glyph) {
            Some((cur, _)) if *cur >= flag => false,
            Some(entry) => {
                *entry = (flag, from.to_string());
                true
            }
            None => {
                self.flags
                    .insert(glyph.to_string(), (flag, from.to_string()));
                true
            }
        }
    }
}

/// The item whose text `line` (a docline index) falls in, or `None` for a
/// document with no items at all. Items are emitted in file order, so their
/// recorded start lines are sorted and the enclosing one is the last that
/// starts at or before `line`.
fn item_at_line(doc: &Document, line: usize) -> Option<usize> {
    doc.item_line_starts
        .partition_point(|&start| start <= line)
        .checked_sub(1)
}

/// Flag every glyph `issues` finds fault with, and everything built out of one.
///
/// `expansion` must be the one `issues` were collected from; it supplies both
/// the concrete glyph names an item expanded to and the `ref` graph the flags
/// travel back along.
#[cfg_attr(all(not(feature = "editor"), not(test)), expect(dead_code))]
pub fn collect(docs: &[&Document], issues: &[Issue], expansion: &Expansion) -> GlyphFlags {
    let mut flags = GlyphFlags::default();
    if issues.is_empty() {
        return flags;
    }

    // Which source item each finding is about. A path that is not in `docs`
    // (an `assert` run against a stale snapshot, a parse error on a file that
    // never became a document) simply flags nothing.
    let by_path: HashMap<&Path, u32> = docs
        .iter()
        .enumerate()
        .map(|(i, d)| (d.path.as_path(), i as u32))
        .collect();
    let mut seeds: HashMap<ItemRef, GlyphFlag> = HashMap::new();
    let mut named: Vec<(&str, GlyphFlag)> = Vec::new();
    for issue in issues {
        let flag = match issue.severity {
            Severity::Error => GlyphFlag::Error,
            Severity::Warning => GlyphFlag::Warning,
            Severity::Todo | Severity::Note => continue,
        };
        // A finding that names its glyph is about that glyph alone, whatever
        // else the line it sits on expands to.
        if let Some(glyph) = &issue.glyph {
            named.push((glyph.as_str(), flag));
            continue;
        }
        let Some(&doc) = by_path.get(issue.file.as_path()) else {
            continue;
        };
        let Some(item) = item_at_line(docs[doc as usize], issue.line) else {
            continue;
        };
        let at = ItemRef {
            doc,
            item: item as u32,
        };
        let slot = seeds.entry(at).or_insert(flag);
        *slot = (*slot).max(flag);
    }
    if seeds.is_empty() && named.is_empty() {
        return flags;
    }

    // `used_by[target]` is every glyph that would carry `target`'s flag.
    let mut used_by: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut glyph_names: HashSet<&str> = HashSet::new();
    // `(glyph, flag, the glyph the flag started at)`.
    let mut queue: Vec<(&str, GlyphFlag, &str)> = Vec::new();
    for e in &expansion.items {
        let DocumentItem::Glyph {
            name: GlyphName(name),
            body,
        } = &e.item
        else {
            continue;
        };
        glyph_names.insert(name);
        for gref in &body.refs {
            used_by.entry(gref.name.as_str()).or_default().push(name);
        }
        if let Some(&flag) = e.origin.and_then(|o| seeds.get(&o))
            && flags.raise(name, flag, name)
        {
            queue.push((name, flag, name));
        }
    }

    // After the loop above, so the propagation queue can hold the expansion's
    // own string rather than the issue's copy of it.
    for (glyph, flag) in named {
        if let Some(&glyph) = glyph_names.get(glyph)
            && flags.raise(glyph, flag, glyph)
        {
            queue.push((glyph, flag, glyph));
        }
    }

    while let Some((glyph, flag, from)) = queue.pop() {
        let Some(users) = used_by.get(glyph) else {
            continue;
        };
        for &user in users {
            if flags.raise(user, flag, from) {
                queue.push((user, flag, from));
            }
        }
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::Resolution;

    fn doc(src: &str) -> Document {
        crate::document_io::parse_document_from_str(src, "t.unf".into()).unwrap()
    }

    fn flags_of(src: &str) -> GlyphFlags {
        let d = doc(src);
        let docs = vec![&d];
        let resolution = Resolution::compute(&docs);
        let issues = crate::issues::collect_issues_with(&docs, &resolution);
        collect(&docs, &issues, &resolution.expansion)
    }

    /// Every glyph here is mapped, so nothing warns for being unused and the
    /// only findings are the ones each case is about.
    const HEAD: &str =
        "meta height 2\nmeta ascent 2\nmeta descent 0\nglyph sq 1 1\n@@\nmap U+0061 = sq\n";

    #[test]
    fn a_glyph_with_a_missing_ref_is_flagged() {
        let flags = flags_of(&format!(
            "{HEAD}\
             glyph a\n\
             ref sq\n\
             map U+0062 = a\n\
             glyph b\n\
             ref nowhere\n\
             map U+0063 = b\n"
        ));
        assert_eq!(flags.get("b"), Some(GlyphFlag::Error));
        assert_eq!(flags.get("a"), None);
    }

    #[test]
    fn the_flag_travels_to_whoever_refs_the_broken_glyph() {
        let flags = flags_of(&format!(
            "{HEAD}\
             glyph a\n\
             ref b\n\
             map U+0062 = a\n\
             glyph b\n\
             ref nowhere\n\
             map U+0063 = b\n"
        ));
        assert_eq!(flags.get("b"), Some(GlyphFlag::Error));
        assert_eq!(flags.get("a"), Some(GlyphFlag::Error));
    }

    #[test]
    fn a_clean_source_flags_nothing() {
        let flags = flags_of(HEAD);
        assert!(flags.is_empty());
    }

    /// The Han shape: one `glyph` line standing for a whole block, each
    /// expansion substituting its own `ref` target. Only the expansions whose
    /// target is missing may be faulted — faulting the line would tint every
    /// character of the block.
    #[test]
    fn one_expansion_of_a_pattern_is_faulted_alone() {
        let flags = flags_of(&format!(
            "{HEAD}\
             glyph part-0061 1 1\n\
             @@\n\
             glyph a-($#0061..0063) 1 1\n\
             ref part-($#0061..0063)\n\
             map U+0061..0063 = a-($#0061..0063)\n"
        ));
        assert_eq!(flags.get("a-0061"), None);
        assert_eq!(flags.get("a-0062"), Some(GlyphFlag::Error));
        assert_eq!(flags.get("a-0063"), Some(GlyphFlag::Error));
    }

    /// …but a fault in what the line *says* — here, that nothing uses it — is
    /// a fault in every glyph it stands for, and carries no name to narrow it
    /// down to one of them.
    #[test]
    fn a_fault_in_the_line_itself_reaches_every_expansion() {
        let flags = flags_of(&format!(
            "{HEAD}\
             glyph part-0061 1 1\n\
             ref sq\n\
             glyph part-0062 1 1\n\
             ref sq\n\
             glyph part-0063 1 1\n\
             ref sq\n\
             glyph a-($#0061..0063) 1 1\n\
             ref part-($#0061..0063)\n"
        ));
        for g in ["a-0061", "a-0062", "a-0063"] {
            assert_eq!(flags.get(g), Some(GlyphFlag::Warning), "{g}");
        }
    }

    /// The Han shape again, from the other side: the tinted cell is the mapped
    /// glyph, but the line to fix is the component's.
    #[test]
    fn an_inherited_flag_names_the_glyph_the_fault_is_in() {
        let flags = flags_of(&format!(
            "{HEAD}\
             glyph a\n\
             ref b\n\
             map U+0062 = a\n\
             glyph b\n\
             ref nowhere\n\
             map U+0063 = b\n"
        ));
        assert_eq!(flags.source("a"), Some("b"));
        assert_eq!(flags.source("b"), Some("b"));
        assert_eq!(flags.source("sq"), None);
    }

    #[test]
    fn an_error_outranks_a_warning_on_the_same_glyph() {
        let mut flags = GlyphFlags::default();
        assert!(flags.raise("a", GlyphFlag::Warning, "a"));
        assert!(flags.raise("a", GlyphFlag::Error, "a"));
        assert!(!flags.raise("a", GlyphFlag::Warning, "a"));
        assert_eq!(flags.get("a"), Some(GlyphFlag::Error));
    }

    #[test]
    fn a_cycle_of_refs_terminates() {
        let flags = flags_of(&format!(
            "{HEAD}\
             glyph a\n\
             ref b\n\
             map U+0062 = a\n\
             glyph b\n\
             ref a\n\
             ref nowhere\n\
             map U+0063 = b\n"
        ));
        assert_eq!(flags.get("a"), Some(GlyphFlag::Error));
        assert_eq!(flags.get("b"), Some(GlyphFlag::Error));
    }
}
