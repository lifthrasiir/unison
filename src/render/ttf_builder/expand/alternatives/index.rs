//! The index of a wide `map` line's alternatives, for consumers that ask per name.

use super::map_spec::MapCharSpec;
use super::*;

/// The `map` alternatives wide enough that asking each of them to produce every
/// name it can is more work than asking every declared name which of them
/// produces it.
///
/// A `PREFIX($-N)SUFFIX` alternative over a range spec names one glyph per code
/// point the line covers — the han slice lines name close to a million between
/// them — while a source-side check only cares about the few thousand a glyph
/// is really declared for. The index turns the question around: it keeps the
/// two literals and the group values the line actually produces, so a candidate
/// name is answered with one `starts_with`, one `ends_with` and one hash
/// lookup, and the walk is over the declared names instead.
///
/// What it cannot invert it does not take, and
/// [`for_each_map_alternative_name`] streams those the forward way — so between
/// the two, every alternative is accounted for exactly once.
#[derive(Default)]
pub(crate) struct MapAlternativeIndex {
    /// Grouped by the literal before the group, which is what a candidate is
    /// tested against first. Wide lines are few by nature — each is thousands
    /// of characters — so scanning the distinct prefixes beats the trie it
    /// would take to avoid scanning them.
    by_prefix: Vec<(String, Vec<IndexedAlt>)>,
}

struct IndexedAlt {
    suffix: String,
    /// Held so the values outlive the line they were read from.
    spec: Arc<MapCharSpec>,
    group: usize,
}

impl MapAlternativeIndex {
    pub(crate) fn is_empty(&self) -> bool {
        self.by_prefix.is_empty()
    }

    /// Take `target` into the index, or report that it has to be streamed.
    pub(super) fn take(&mut self, target: &AltTarget<'_>, spec: Option<&Arc<MapCharSpec>>) -> bool {
        let AltTarget::Capture {
            prefix,
            suffix,
            group_idx,
            ..
        } = target
        else {
            return false;
        };
        let Some(spec) = spec else { return false };
        if !spec.produced.get(*group_idx).is_some_and(Option::is_some) {
            return false;
        }
        let alt = IndexedAlt {
            suffix: (*suffix).to_string(),
            spec: spec.clone(),
            group: *group_idx,
        };
        match self.by_prefix.iter_mut().find(|(p, _)| p == prefix) {
            Some((_, alts)) => alts.push(alt),
            None => self.by_prefix.push(((*prefix).to_string(), vec![alt])),
        }
        true
    }

    /// Whether any alternative the index took names `name`.
    pub(crate) fn produces(&self, name: &str) -> bool {
        self.by_prefix.iter().any(|(prefix, alts)| {
            name.starts_with(prefix.as_str())
                && alts.iter().any(|alt| {
                    name.len() >= prefix.len() + alt.suffix.len()
                        && name.ends_with(alt.suffix.as_str())
                        && alt.spec.produced[alt.group].as_ref().is_some_and(|values| {
                            values.contains(&name[prefix.len()..name.len() - alt.suffix.len()])
                        })
                })
        })
    }
}
