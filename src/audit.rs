//! The `audit` directive: rules the *source* is held to, stated once for a
//! whole family of glyphs.
//!
//! # Why this is not `meta`
//!
//! [`crate::meta`] states what goes *into the font file* — a name record, a
//! metric, a PANOSE vector. An `audit` line states nothing that a consumer of
//! the font could ever read: it says what the source is supposed to look like,
//! so that a drawing which drifts from it is reported rather than shipped.
//! Keeping the two apart matters because their failure modes are opposite. A
//! `meta` key that goes missing changes the font; an `audit` rule that goes
//! missing changes nothing except that nobody is told any more.
//!
//! It is a *global* rule and not a per-glyph flag for the same reason a
//! stylesheet is not an inline style: the whole point is that 20k glyphs are
//! held to one standard, and a rule restated per glyph is a rule that drifts.
//! The prefix is how a source says which family of glyphs it means.
//!
//! # The line
//!
//! `audit KEY ARGUMENT…`, one key per line — the same shape as `meta`, and for
//! the same reason (keys are variadic, so two on one line could not be told
//! apart). There is no face scope: a face selects which characters map to which
//! glyph, never how a glyph is drawn, and every face draws from one glyph set.
//!
//! The keys:
//!
//! ```text
//! audit ideal-clearance han-* 0 1        // one band for every IDC line
//! audit ideal-clearance han-* 0 1 1 2    // …or a second one for enclosures
//! audit max-contact-run han-* 2
//! ```
//!
//! Everything here is single-assignment, exactly as `meta` is: setting one slot
//! twice is an error even when the two values agree, reported by
//! [`crate::issues`]. Which slot a line assigns to is [`AuditEntry::slot`] —
//! for `ideal-clearance` that is one slot *per prefix*, since stating a band
//! for `han-*` and a tighter one for a subset of it is the intended use rather
//! than a conflict.

use std::collections::BTreeMap;

use crate::document::{Document, DocumentItem};

/// One parsed `audit` line. The variant *is* the key, so a key that parses is
/// a key some consumer handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuditEntry {
    /// `ideal-clearance PREFIX* MIN MAX [MIN MAX]` — how much room an IDC
    /// line's parts are meant to leave each other and the glyph's edges. The
    /// second pair, where a source writes one, is the band an *enclosure* line
    /// is held to; with one pair both kinds of line share it. See
    /// [`IdealClearances`], [`ClearanceBand`] and [`crate::compose`].
    IdealClearance { prefix: String, band: ClearanceBand },
    /// `max-contact-run PREFIX* N` — how many consecutive lines two neighbouring
    /// parts of an IDC line may touch along before the layout owes them a cell
    /// of clearance. See [`MaxContactRuns`] and [`crate::compose::contact_run`].
    MaxContactRun { prefix: String, max: u16 },
}

impl AuditEntry {
    /// The slot this line assigns to, as duplicate detection sees it.
    pub fn slot(&self) -> String {
        match self {
            // One slot per prefix: several rules are the point (a `han-*` band
            // and a tighter one for a subset), so only the same prefix twice
            // is a duplicate.
            Self::IdealClearance { prefix, .. } => format!("ideal-clearance {prefix}"),
            Self::MaxContactRun { prefix, .. } => format!("max-contact-run {prefix}"),
        }
    }

    /// How the slot is worth naming in a diagnostic.
    pub fn describe_slot(&self) -> String {
        format!("`audit {}`", self.slot())
    }
}

/// Every key, for the unknown-key message.
fn known_keys() -> String {
    "ideal-clearance, max-contact-run".to_string()
}

/// The `PREFIX*` every key is scoped by: a glyph name's front, so a `*` that is
/// anywhere but the end would match by something this never asks.
fn check_prefix(key: &str, prefix: &str) -> Result<(), String> {
    if let Some(pos) = prefix.find('*')
        && pos + 1 != prefix.len()
    {
        return Err(format!(
            "`audit {key}` matches a glyph name by its front, so `*` may only be \
             the last character of `{prefix}`",
        ));
    }
    if prefix.matches('*').count() > 1 {
        return Err(format!(
            "`audit {key}` takes one `*`, at the end; `{prefix}` has more",
        ));
    }
    Ok(())
}

/// Parse the text of an [`DocumentItem::Audit`] item — everything after the
/// keyword, comment included.
///
/// `Err` carries a message ready to report; the caller supplies the position.
/// A key that takes N values rejects N±1: a rule that is quietly half-read is
/// a rule that quietly stops checking, which is worse than no rule at all.
pub fn parse_audit_entry(text: &str) -> Result<AuditEntry, String> {
    let tokens = crate::document_io::tokenize_tokens(text)
        .map_err(|e| format!("malformed `audit` line: {e}"))?;
    let Some((key, rest)) = tokens.split_first() else {
        return Err("`audit` needs a key".to_string());
    };
    match key.as_str() {
        "ideal-clearance" => {
            // Two pairs or one, and nothing between: a one-dimensional split
            // and an enclosure leave room for different reasons — an enclosure
            // has to fit a whole drawing inside another one — so a source that
            // wants them held apart says so, and one that does not writes the
            // band once. What is refused is 3 or 5 values, which is a source
            // half-way through saying something.
            let (prefix, values) = match rest {
                [prefix, rest @ ..] if rest.len() == 2 || rest.len() == 4 => (prefix, rest),
                _ => {
                    return Err(format!(
                        "`audit {key}` takes a glyph-name prefix and 2 or 4 values, got {}",
                        rest.len(),
                    ));
                }
            };
            check_prefix(key, prefix)?;
            let num = |v: &String| {
                v.parse::<i16>()
                    .map_err(|_| format!("`audit {key}` takes numbers, got `{v}`"))
            };
            let mut pairs = Vec::new();
            for pair in values.chunks(2) {
                let (min, max) = (num(&pair[0])?, num(&pair[1])?);
                if min > max {
                    return Err(format!(
                        "`audit {key}` range is {min}..{max}, which is empty"
                    ));
                }
                pairs.push((min, max));
            }
            let linear = pairs[0];
            Ok(AuditEntry::IdealClearance {
                prefix: prefix.clone(),
                band: ClearanceBand {
                    linear,
                    // One pair states one standard for every IDC line, which is
                    // what a source that has never drawn an enclosure means.
                    enclosing: pairs.get(1).copied().unwrap_or(linear),
                },
            })
        }
        "max-contact-run" => {
            let [prefix, max] = rest else {
                return Err(format!(
                    "`audit {key}` takes a glyph-name prefix and 1 value, got {}",
                    rest.len(),
                ));
            };
            check_prefix(key, prefix)?;
            let max = max
                .parse::<u16>()
                .map_err(|_| format!("`audit {key}` takes a count of lines, got `{max}`"))?;
            Ok(AuditEntry::MaxContactRun {
                prefix: prefix.clone(),
                max,
            })
        }
        _ => Err(format!(
            "unknown `audit` key `{key}` (known keys: {})",
            known_keys()
        )),
    }
}

/// Every `audit` rule a document set states.
///
/// Malformed and duplicate lines are *not* reported here, exactly as they are
/// not in [`crate::meta::FontMeta`]: this runs wherever the rules are needed,
/// while reporting belongs to [`crate::issues`]. Both sides share
/// [`parse_audit_entry`], so they cannot disagree about what a line means; a
/// line that fails to parse is skipped.
#[derive(Clone, Debug, Default)]
pub struct AuditRules {
    pub ideal_clearance: IdealClearances,
    pub max_contact_run: MaxContactRuns,
}

impl AuditRules {
    pub fn collect(docs: &[&Document]) -> Self {
        let mut rules = Self::default();
        for doc in docs {
            for item in &doc.items {
                let DocumentItem::Audit(text) = item else {
                    continue;
                };
                match parse_audit_entry(text) {
                    Ok(AuditEntry::IdealClearance { prefix, band }) => {
                        rules.ideal_clearance.rules.insert(prefix, band);
                    }
                    Ok(AuditEntry::MaxContactRun { prefix, max }) => {
                        rules.max_contact_run.rules.insert(prefix, max);
                    }
                    Err(_) => continue,
                }
            }
        }
        rules
    }
}

/// The rules of one `audit` key, and which one a glyph is held to.
///
/// Rules are keyed by the pattern as written (`han-*`, or a bare name matching
/// exactly one glyph), which is also the duplicate-detection slot, so a source
/// may state as many as it likes. When more than one matches a name the
/// **longest** wins and an exact name beats every prefix: that is what makes a
/// rule for one troublesome glyph an exception rather than a second answer.
///
/// Every key scopes the same way, so the matching lives here once; what a rule
/// *says* is the type parameter.
#[derive(Clone, Debug)]
pub struct PrefixRules<T> {
    /// Pattern as written -> what it states.
    rules: BTreeMap<String, T>,
}

// Hand-written rather than derived: a derived `Default` would demand one of
// `T` as well, and "no rules at all" says nothing about what a rule states.
impl<T> Default for PrefixRules<T> {
    fn default() -> Self {
        Self {
            rules: BTreeMap::new(),
        }
    }
}

impl<T> PrefixRules<T> {
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// The rule `glyph` is held to: the pattern it was written as and what it
    /// states, or `None` when no rule reaches the name.
    pub fn get(&self, glyph: &str) -> Option<(&str, &T)> {
        self.rules
            .iter()
            .filter_map(|(pattern, value)| {
                let specificity = match pattern.strip_suffix('*') {
                    Some(prefix) => glyph.starts_with(prefix).then_some(prefix.len()),
                    // An exact name is more specific than any prefix of it.
                    None => (pattern == glyph).then_some(usize::MAX),
                }?;
                Some((specificity, pattern.as_str(), value))
            })
            .max_by_key(|&(specificity, ..)| specificity)
            .map(|(_, pattern, value)| (pattern, value))
    }
}

/// The `audit ideal-clearance` rules: how much room an IDC line's parts are
/// meant to leave each other and the glyph's edges, as an inclusive range.
///
/// See [`crate::compose`] for what a clearance is and where it is measured;
/// violating one is a warning, since it is a drawing that has drifted rather
/// than a source that cannot be built.
pub type IdealClearances = PrefixRules<ClearanceBand>;

/// What one `audit ideal-clearance` rule states: the band a one-dimensional
/// split is held to, and the band an enclosure is.
///
/// They are two numbers rather than one because the two layouts spend room on
/// different things. A `⿰` junction is two parts standing beside each other and
/// a cell between them is generous; an enclosure has to seat a whole drawing
/// inside the cavity of another, where the same cell is often the difference
/// between "inside" and "wedged". A source that has not drawn an enclosure yet
/// writes one pair and both read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearanceBand {
    pub linear: (i16, i16),
    pub enclosing: (i16, i16),
}

impl ClearanceBand {
    /// The band the line at hand is held to.
    pub fn range(&self, enclosing: bool) -> (i16, i16) {
        if enclosing {
            self.enclosing
        } else {
            self.linear
        }
    }
}

/// The `audit max-contact-run` rules: how many consecutive lines two
/// neighbouring parts may touch along before the layout owes them a cell.
///
/// This is the one rule that reads the *pattern* of two facing edges rather
/// than the distance between them, which is what tells a pair of flat faces
/// (which want parting) from a pair that meets at a tip (which does not). It
/// says nothing about hardblanks and needs none: a hardblank that has already
/// parted two parts leaves no ink touching for this to count. See
/// [`crate::compose::contact_run`].
///
/// It says its piece *as* a clearance — the junction reports a cell less — so
/// it is in force only where an `audit ideal-clearance` rule reaches the same
/// glyph. A source stating this key alone measures nothing, which is the same
/// arrangement every other consumer of an ink profile is under: nothing is
/// profiled where nothing holds the result to anything.
pub type MaxContactRuns = PrefixRules<u16>;

impl IdealClearances {
    /// The pattern the rule was written as and what it states. Which of the
    /// two bands applies is [`ClearanceBand::range`], asked with the operator
    /// the line at hand carries.
    pub fn for_glyph(&self, glyph: &str) -> Option<(&str, &ClearanceBand)> {
        self.get(glyph)
    }
}

impl MaxContactRuns {
    /// The pattern the rule was written as and the longest tolerated run.
    pub fn for_glyph(&self, glyph: &str) -> Option<(&str, u16)> {
        self.get(glyph).map(|(p, &max)| (p, max))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document_io::parse_document_from_str;

    fn rules_of(src: &str) -> AuditRules {
        let doc = parse_document_from_str(src, "test.unf".into()).unwrap();
        AuditRules::collect(&[&doc])
    }

    #[test]
    fn an_ideal_clearance_rule_parses() {
        assert_eq!(
            parse_audit_entry("ideal-clearance han-* 0 1").unwrap(),
            AuditEntry::IdealClearance {
                prefix: "han-*".to_string(),
                band: ClearanceBand {
                    linear: (0, 1),
                    // One pair is one standard for every IDC line.
                    enclosing: (0, 1),
                },
            },
        );
        // A range may admit an overlap.
        assert!(matches!(
            parse_audit_entry("ideal-clearance han-* -1 1"),
            Ok(AuditEntry::IdealClearance { band, .. }) if band.linear == (-1, 1)
        ));
        // The `*` matches a name's front, so it can only be at the end.
        assert!(
            parse_audit_entry("ideal-clearance han-*-l 0 1")
                .unwrap_err()
                .contains("last character"),
        );
        assert!(
            parse_audit_entry("ideal-clearance han-* 1 0")
                .unwrap_err()
                .contains("empty")
        );
        assert!(parse_audit_entry("ideal-clearance han-* 0").is_err());
        assert!(parse_audit_entry("clearance han-* 0 1").is_err());
        assert!(parse_audit_entry("").is_err());
    }

    #[test]
    fn an_ideal_clearance_rule_may_state_the_enclosure_band_apart() {
        let entry = parse_audit_entry("ideal-clearance han-* 0 1 1 2").unwrap();
        let AuditEntry::IdealClearance { band, .. } = entry else {
            panic!("not an ideal-clearance rule");
        };
        assert_eq!(band.linear, (0, 1));
        assert_eq!(band.enclosing, (1, 2));
        assert_eq!(band.range(false), (0, 1));
        assert_eq!(band.range(true), (1, 2));
        // Either range may be empty, and either says so.
        assert!(
            parse_audit_entry("ideal-clearance han-* 0 1 2 1")
                .unwrap_err()
                .contains("empty")
        );
        // Half of a second pair is a source that stopped mid-sentence.
        assert!(parse_audit_entry("ideal-clearance han-* 0 1 2").is_err());
        assert!(parse_audit_entry("ideal-clearance han-* 0 1 2 3 4").is_err());
    }

    #[test]
    fn a_max_contact_run_rule_parses() {
        assert_eq!(
            parse_audit_entry("max-contact-run han-* 2").unwrap(),
            AuditEntry::MaxContactRun {
                prefix: "han-*".to_string(),
                max: 2,
            },
        );
        // 0 is a rule like any other: no two parts may touch at all.
        assert!(matches!(
            parse_audit_entry("max-contact-run han-* 0"),
            Ok(AuditEntry::MaxContactRun { max: 0, .. })
        ));
        // A count of lines, so nothing below zero and nothing fractional.
        assert!(parse_audit_entry("max-contact-run han-* -1").is_err());
        assert!(parse_audit_entry("max-contact-run han-* two").is_err());
        assert!(parse_audit_entry("max-contact-run han-*").is_err());
        assert!(parse_audit_entry("max-contact-run han-* 1 2").is_err());
        // The prefix rule is the one every key shares.
        assert!(
            parse_audit_entry("max-contact-run han-*-l 2")
                .unwrap_err()
                .contains("last character"),
        );
        // Its own key, so it neither collides with nor replaces the other one.
        let r = rules_of("audit ideal-clearance han-* 0 1\naudit max-contact-run han-* 2\n");
        assert_eq!(
            r.ideal_clearance
                .for_glyph("han-4e00")
                .map(|(p, b)| (p, b.linear)),
            Some(("han-*", (0, 1)))
        );
        assert_eq!(r.max_contact_run.for_glyph("han-4e00"), Some(("han-*", 2)));
        assert!(r.max_contact_run.for_glyph("latin-a").is_none());
        assert_ne!(
            parse_audit_entry("max-contact-run han-* 2").unwrap().slot(),
            parse_audit_entry("ideal-clearance han-* 0 1")
                .unwrap()
                .slot(),
        );
    }

    /// One slot per prefix, so a source may hold different families of glyph to
    /// different ranges — only the same prefix twice is a duplicate.
    #[test]
    fn each_prefix_is_its_own_slot() {
        let slot = |text: &str| parse_audit_entry(text).unwrap().slot();
        assert_ne!(
            slot("ideal-clearance han-* 0 1"),
            slot("ideal-clearance hang-* 0 1"),
        );
        assert_eq!(
            slot("ideal-clearance han-* 0 1"),
            slot("ideal-clearance han-* 2 3"),
        );
    }

    #[test]
    fn the_most_specific_clearance_rule_wins() {
        let r = rules_of(
            "audit ideal-clearance han-* 0 1\n\
             audit ideal-clearance han-6c* 1 2\n\
             audit ideal-clearance han-4e00 3 4\n",
        )
        .ideal_clearance;
        let band = |g: &str| r.for_glyph(g).map(|(p, b)| (p, b.linear));
        assert_eq!(band("han-53ef"), Some(("han-*", (0, 1))));
        assert_eq!(band("han-6c35"), Some(("han-6c*", (1, 2))));
        // A bare name is a rule for that one glyph, and beats every prefix of it.
        assert_eq!(band("han-4e00"), Some(("han-4e00", (3, 4))));
        assert_eq!(band("latin-a"), None);
        assert!(rules_of("").ideal_clearance.is_empty());
    }

    /// A line that does not parse leaves no rule behind — the report comes
    /// from `issues.rs`, and until it is fixed nothing is held to it.
    #[test]
    fn an_unreadable_line_states_no_rule() {
        assert!(
            rules_of("audit ideal-clearance han-* one two\n")
                .ideal_clearance
                .is_empty()
        );
    }
}
