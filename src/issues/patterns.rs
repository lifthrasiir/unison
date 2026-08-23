//! Two checks over what a line states rather than over what it built: a
//! name pattern whose groups are ragged, and a `prop` line whose property
//! values are not ones the UCD uses.

use crate::document::{Document, DocumentItem, substitute_name_parts};
use crate::pattern::{NamePartsMap, NamePattern};
use crate::pattern::{capture_groups, substitute_name_parts_and_captures};

use super::{Issue, Severity, issue_at};

/// The names written on one item, each with whether it is read as a glyph
/// *block* name.  That is the one context where a top-level `|` is a verbatim
/// list rather than an alternation group ([`crate::pattern`]), so the two
/// cannot be parsed alike.
fn written_patterns(item: &DocumentItem) -> Vec<(&str, bool)> {
    match item {
        // An IDC component expands in lock-step with the block's name exactly
        // as a `ref` target does, so a ragged group is the same fault there.
        DocumentItem::Glyph { name, body } => std::iter::once((name.0.as_str(), true))
            .chain(body.refs.iter().map(|r| (r.name.as_str(), false)))
            .chain(
                body.compose
                    .iter()
                    .flat_map(|c| c.part_names())
                    .map(|p| (p, false)),
            )
            .collect(),
        DocumentItem::GlyphAlias { name, target, .. } => {
            vec![(name.0.as_str(), true), (target.as_str(), false)]
        }
        DocumentItem::Map { glyph, .. } => vec![(glyph.as_str(), false)],
        DocumentItem::MapDecomposed { glyph, .. } => {
            glyph.as_deref().map(|g| (g, false)).into_iter().collect()
        }
        DocumentItem::Remap { .. } => item.remap_operands().map(|s| (s.as_str(), false)).collect(),
        _ => Vec::new(),
    }
}

/// The groups an item's leading pattern writes, which every other name on it
/// may name with a `$-N`. The check has to substitute them like the expansion
/// does, or it measures a one-alternative `($-1)` instead of the group it
/// stands for and never sees the ragged case.
fn item_captures(item: &DocumentItem, name_parts: &NamePartsMap) -> Vec<Vec<String>> {
    match item {
        DocumentItem::Glyph { name, .. } | DocumentItem::GlyphAlias { name, .. } => {
            capture_groups(&substitute_name_parts(&name.0, name_parts))
        }
        DocumentItem::Map {
            char_repr,
            selector,
            ..
        } => crate::render::ttf_builder::map_char_captures(char_repr, selector.as_deref()),
        DocumentItem::MapDecomposed { char_repr, .. } => {
            crate::render::ttf_builder::map_char_captures(char_repr, None)
        }
        _ => Vec::new(),
    }
}

fn join_counts(lens: &[usize]) -> String {
    let parts: Vec<String> = lens.iter().map(|n| n.to_string()).collect();
    match parts.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// Groups and lock-step operands that do not divide the expansion they sit in.
///
/// The expansion is as long as its largest group and every other group cycles
/// inside it, so a group whose size does not divide that length stops partway
/// through its own cycle and the combinations past the cut are never written.
/// Nothing downstream can tell that from a deliberate cycle — the names it does
/// produce are all valid — so it is caught here, at the line that writes it.
///
/// This is the check that replaced the old LCM rule, which had no ragged case
/// to report because it grew the expansion until every group divided it. That
/// made more patterns come out right and the wrong ones invisible: one more
/// alternative could halve the expansion with nothing to see.
///
/// A `remap`'s lookbehind and lookahead are deliberately not part of this. They
/// expand to independent alternative *sets* (one coverage each), not to
/// positions indexed in lock-step with the rule's entries, so their sizes have
/// nothing to divide.
pub(super) fn check_ragged_patterns(
    docs: &[&Document],
    name_parts: &NamePartsMap,
    issues: &mut Vec<Issue>,
) {
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            let captures = item_captures(item, name_parts);
            for (written, is_block) in written_patterns(item) {
                // The leading pattern is the one that *declares* the groups, so
                // it never names them.
                let substituted = if is_block {
                    substitute_name_parts(written, name_parts)
                } else {
                    substitute_name_parts_and_captures(written, name_parts, &captures)
                };
                let parsed = if is_block {
                    NamePattern::parse(&substituted)
                } else {
                    NamePattern::parse_element(&substituted)
                };
                // A pattern that does not parse, or that is over the expansion
                // limit, is already an error from the resolution pass.
                let Ok(parsed) = parsed else { continue };
                let ragged = parsed.ragged_group_lens();
                if ragged.is_empty() {
                    continue;
                }
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Warning,
                    format!(
                        "name pattern '{written}' expands to {} names, but its group of {} \
                         alternatives does not divide that, so it repeats partway and the \
                         remaining combinations are never written; for a full cross product \
                         repeat each alternative with the `**N` group multiplier",
                        parsed.len(),
                        join_counts(&ragged),
                    ),
                ));
            }

            // Across a rule's operands the same rule holds, one step up: the
            // entry count is the longest of them and each one tiles it.
            let DocumentItem::Remap { source, target, .. } = item else {
                continue;
            };
            let operands: Vec<(&String, NamePattern)> = source
                .iter()
                .chain(target)
                .map(|s| (s, crate::pattern::parse_name_element(s, name_parts)))
                .collect();
            let entries = crate::pattern::combined_len(operands.iter().map(|(_, p)| p));
            for (written, parsed) in &operands {
                if parsed.is_empty() || entries % parsed.len() == 0 {
                    continue;
                }
                issues.push(issue_at(
                    doc,
                    item_idx,
                    Severity::Warning,
                    format!(
                        "remap operand '{written}' denotes {} names, which does not divide the \
                         {entries} entries this rule expands to, so it repeats partway; the \
                         entry count is the longest operand, and every other one has to tile it",
                        parsed.len(),
                    ),
                ));
            }
        }
    }
}

/// `prop` lines: the property values have to be the ones the UCD uses, and a
/// line has to actually cover a character.
///
/// A wrong `gc`/`eaw` is an error rather than a warning even though nothing in
/// the font depends on it: the whole point of the line is to be *read*, and a
/// value the UCD does not use is one no reader can check against anything.
pub(super) fn check_props(docs: &[&Document], issues: &mut Vec<Issue>) {
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                DocumentItem::PropChar {
                    char_repr, values, ..
                } => {
                    if let Some(gc) = &values.gc
                        && !crate::ucd::GENERAL_CATEGORIES.contains(&gc.as_str())
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "`gc {gc}` is not a General_Category short name (Lu, Ll, Lo, Mn, \
                                 So, …)"
                            ),
                        ));
                    }
                    if let Some(eaw) = &values.eaw
                        && !crate::ucd::EAST_ASIAN_WIDTHS.contains(&eaw.as_str())
                    {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!(
                                "`eaw {eaw}` is not an East_Asian_Width short name \
                                 (N, Na, A, W, F, H)"
                            ),
                        ));
                    }
                    // The same expansion `CharProps` performs, so a line that
                    // states nothing there says so here rather than being
                    // quietly absent from every status bar.
                    let pairs = crate::render::ttf_builder::expand_map_pairs(char_repr, "");
                    if pairs.is_empty() {
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Error,
                            format!("prop `{char_repr}` names no character"),
                        ));
                    }
                    for (cp, _) in pairs {
                        if char::from_u32(cp).is_none() {
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Error,
                                format!(
                                    "prop `{char_repr}`: U+{cp:04X} is not a valid Unicode \
                                     scalar value"
                                ),
                            ));
                            break;
                        }
                    }
                }
                DocumentItem::PropBlock { name, end, .. } if *end > 0x10FFFF => {
                    issues.push(issue_at(
                        doc,
                        item_idx,
                        Severity::Error,
                        format!("prop block `{name}` ends past U+10FFFF"),
                    ));
                }
                _ => {}
            }
        }
    }
}
