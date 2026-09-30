//! Settling of wide `map` groups: rows shared between alternatives, and the emission of what settled.

use super::*;

/// Every row of one wide `map` line, settled together with the other lines that
/// write the same character spec.
pub(super) struct WideGroupSettle {
    /// The group's spec, kept so the line can rebuild its targets to read a
    /// settled index back out. Shared, because an identical character spec
    /// expands to identical rows — which is what makes the group one.
    pub(super) spec: std::sync::Arc<WideMapRows>,
    pub(super) settled: Vec<Option<SettledAlt>>,
}

/// Settle the wide `map` lines that write the *same* character spec in one
/// pass, so that a name is judged once per row rather than once per line.
///
/// A font that varies a glyph family by region writes the family once per
/// region — seven `map han-XX : U+($#4e00..9fff) = han-($-1) han-($-1)-g …`
/// lines, the same nine alternatives permuted. Line by line that asks whether
/// `han-4e00-g` names a glyph seven times, and settling alternatives is the
/// largest single stage of an expansion; the answer cannot differ between the
/// lines, because [`usable`](resolve_map_alternatives) is a pure function of
/// the glyph set. So each row judges every distinct name its group's lines name
/// once and hands the answer to all of them.
///
/// Only groups of two or more are collected: a line with no partner would pay
/// for the memo and read it once, and takes the straight path instead. A spec
/// wide enough to matter is also parsed and expanded once for the group rather
/// than once per line.
pub(super) fn settle_wide_groups(
    items: &[ExpandedItem],
    resolvable: &impl Fn(&ExpandedItem) -> bool,
    usable: &(impl Fn(&str) -> bool + Sync),
    notdef_usable: bool,
    cancel: &crate::cancel::CancelToken,
) -> HashMap<usize, WideGroupSettle> {
    let mut groups: HashMap<&str, Vec<usize>> = HashMap::default();
    for (idx, e) in items.iter().enumerate() {
        if !resolvable(e) {
            continue;
        }
        let DocumentItem::Map {
            char_repr,
            selector: None,
            ..
        } = &e.item
        else {
            continue;
        };
        groups.entry(char_repr.as_str()).or_default().push(idx);
    }
    groups.retain(|_, members| members.len() > 1);

    let mut out: HashMap<usize, WideGroupSettle> = HashMap::default();
    for (char_repr, members) in groups {
        let Some(spec) = wide_map_rows(char_repr) else {
            continue;
        };
        if spec.rows().is_empty() {
            continue;
        }
        let spec = std::sync::Arc::new(spec);
        // The group's alternatives, each distinct written target once, and
        // every line as indices into them. The lines permute one list, and a
        // target written the same way over the same spec names the same glyph
        // at every row — so the memo is keyed by the target, not by the name
        // it builds, and a name is neither built nor compared twice.
        //
        // Borrowed from `items` for as long as the settling below, which is why
        // this is a pass of its own: the caller consumes `items` as it emits.
        let mut written: Vec<&str> = Vec::new();
        let lines: Vec<(Vec<usize>, bool)> = members
            .iter()
            .map(|&idx| {
                let DocumentItem::Map { glyphs, .. } = &items[idx].item else {
                    unreachable!("collected as a map above");
                };
                let alts = glyphs
                    .iter()
                    .map(|g| match written.iter().position(|w| w == g) {
                        Some(t) => t,
                        None => {
                            written.push(g);
                            written.len() - 1
                        }
                    })
                    .collect();
                (alts, glyphs.last().is_some_and(String::is_empty))
            })
            .collect();
        let targets: Vec<AltTarget<'_>> = written.iter().map(|g| AltTarget::of(g, &spec)).collect();

        let rows = spec.rows();
        let per_row = crate::parallel::map_indexed(rows.len(), cancel, |k| {
            let (i, _) = rows[k];
            // The row's memo, one slot per distinct target. Per row rather
            // than shared across rows, because sharing would need a lock on
            // the one loop that must not have one — see `crate::parallel`.
            let mut judged: Vec<Option<bool>> = vec![None; targets.len()];
            let mut buf = String::new();
            lines
                .iter()
                .map(|(alts, optional)| {
                    settle_alt(
                        alts.len(),
                        |n| {
                            let t = alts[n];
                            *judged[t].get_or_insert_with(|| {
                                targets[t].get_into(i, &mut buf);
                                usable(&buf)
                            })
                        },
                        *optional,
                        notdef_usable,
                    )
                })
                .collect::<Vec<_>>()
        });

        // Row-major to line-major: each line is emitted on its own, in the
        // order the source wrote it.
        let mut settled: Vec<Vec<Option<SettledAlt>>> = members
            .iter()
            .map(|_| Vec::with_capacity(rows.len()))
            .collect();
        for row in per_row {
            for (line, answer) in settled.iter_mut().enumerate() {
                answer.push(row.as_ref().map(|per_line| per_line[line]));
            }
        }
        for (&idx, settled) in members.iter().zip(settled) {
            out.insert(
                idx,
                WideGroupSettle {
                    spec: spec.clone(),
                    settled,
                },
            );
        }
    }
    out
}

/// Turn one wide line's settled rows into the `map` items they stand for, and
/// return what [`report_unmatched`] needs — the first row that matched nothing
/// and how many more followed it.
pub(super) fn emit_settled_rows(
    rows: &[(usize, u32)],
    settled: &[Option<SettledAlt>],
    targets: &[AltTarget<'_>],
    slices: &[String],
    origin: Option<ItemRef>,
    out: &mut Vec<ExpandedItem>,
) -> Option<(String, usize)> {
    let mut unmatched: Option<(String, usize)> = None;
    for (&(i, cp), settled) in rows.iter().zip(settled) {
        let Some(settled) = settled else {
            continue;
        };
        let (chosen, matched) = match settled {
            SettledAlt::Alt(n) => (Some(targets[*n as usize].get(i)), true),
            SettledAlt::Dropped => (None, true),
            SettledAlt::Notdef => (Some(NOTDEF.to_string()), false),
            SettledAlt::Nothing => (None, false),
        };
        if !matched {
            match &mut unmatched {
                Some((_, more)) => *more += 1,
                None => unmatched = Some((format!("U+{cp:04X}"), 0)),
            }
        }
        if let Some(chosen) = chosen {
            out.push(ExpandedItem {
                item: DocumentItem::Map {
                    slices: slices.to_vec(),
                    char_repr: format!("U+{cp:04X}"),
                    selector: None,
                    glyphs: vec![chosen],
                    comment: None,
                },
                origin,
            });
        }
    }
    unmatched
}
