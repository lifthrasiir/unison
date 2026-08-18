//! `remap group`: what a declaration says about the lookup as a whole, and the
//! stable order the groups are built in.

use std::collections::HashMap;

use super::{Document, DocumentItem};

/// What a `remap group` declaration says about the lookup as a whole.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemapGroupInfo {
    pub reversed: bool,
    pub after: Vec<String>,
    /// False for a group that only ever appeared as a rule's group name.
    pub declared: bool,
    /// False for a group that has a declaration and no rules, which builds no
    /// lookup at all.
    pub has_rules: bool,
}

/// The order every remap group's lookup is built in, and what went wrong
/// working it out. Both the builder and [`crate::issues`] read this, so the
/// order the font is built with and the order the report complains about
/// cannot drift apart.
#[derive(Clone, Debug, Default)]
pub struct RemapGroupOrder {
    /// Groups in lookup order. Every group named anywhere appears exactly once,
    /// including those tangled in a cycle.
    pub order: Vec<String>,
    pub info: HashMap<String, RemapGroupInfo>,
    /// `after` targets that name no group, as (group, missing target).
    pub unknown_after: Vec<(String, String)>,
    /// Groups whose `after` constraints could not all be honoured because they
    /// form a cycle, in source order. Their relative order falls back to that.
    pub cycle: Vec<String>,
    /// Groups declared by more than one `remap group` line.
    pub duplicate_decls: Vec<String>,
}

/// Order remap groups by source position, then let `after` move them.
///
/// The sort is a stable topological one: among the groups whose constraints are
/// already satisfied it always takes the earliest in source order, so adding an
/// `after` to one group leaves every unrelated group exactly where it was. That
/// stability is the whole point — without it the lookup indices of a font would
/// shuffle on an unrelated edit.
pub fn remap_group_order(docs: &[&Document]) -> RemapGroupOrder {
    let mut out = RemapGroupOrder::default();
    let mut index: HashMap<String, usize> = HashMap::new();

    let see = |name: &str, out: &mut RemapGroupOrder, index: &mut HashMap<String, usize>| {
        if !index.contains_key(name) {
            index.insert(name.to_string(), out.order.len());
            out.order.push(name.to_string());
            out.info.insert(name.to_string(), RemapGroupInfo::default());
        }
    };

    for doc in docs {
        for item in &doc.items {
            match item {
                DocumentItem::Remap { feature, .. } => {
                    see(feature, &mut out, &mut index);
                    out.info.get_mut(feature).expect("just inserted").has_rules = true;
                }
                DocumentItem::RemapGroup {
                    name,
                    reversed,
                    after,
                    ..
                } => {
                    see(name, &mut out, &mut index);
                    let info = out.info.get_mut(name).expect("just inserted");
                    if info.declared {
                        out.duplicate_decls.push(name.clone());
                    } else {
                        info.declared = true;
                        info.reversed = *reversed;
                        info.after = after.clone();
                    }
                }
                _ => {}
            }
        }
    }

    // An `after` may name a group declared further down, so the targets can
    // only be resolved once every group is known.
    let source_order = std::mem::take(&mut out.order);
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); source_order.len()];
    for (i, name) in source_order.iter().enumerate() {
        for target in &out.info[name].after.clone() {
            match index.get(target) {
                Some(&t) if t != i => deps[i].push(t),
                // Naming itself is a one-node cycle; leaving the edge out would
                // quietly turn it into a no-op instead.
                Some(_) => deps[i].push(i),
                None => out.unknown_after.push((name.clone(), target.clone())),
            }
        }
    }

    let mut emitted = vec![false; source_order.len()];
    let mut order = Vec::with_capacity(source_order.len());
    while order.len() < source_order.len() {
        let ready =
            (0..source_order.len()).find(|&i| !emitted[i] && deps[i].iter().all(|&d| emitted[d]));
        match ready {
            Some(i) => {
                emitted[i] = true;
                order.push(source_order[i].clone());
            }
            // Nothing is ready and something is left: the rest is one or more
            // cycles. Emit them in source order so the font still builds, and
            // let the report name them.
            None => {
                for i in 0..source_order.len() {
                    if !emitted[i] {
                        emitted[i] = true;
                        out.cycle.push(source_order[i].clone());
                        order.push(source_order[i].clone());
                    }
                }
            }
        }
    }

    out.order = order;
    out
}

// ---------------------------------------------------------------------------
// Name-parts collection
// ---------------------------------------------------------------------------
