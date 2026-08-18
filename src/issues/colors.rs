//! Colors a `color` alias or a `ref ... fill` names but nothing declares.

use crate::document::DocumentItem;

use super::{Cx, Issue, Severity, issue_at};

pub(super) fn check_colors(cx: &Cx<'_>, issues: &mut Vec<Issue>) {
    let docs = cx.docs;
    let _aliases = cx.aliases;
    // A `color` alias or a `ref ... fill` naming a color nothing declares
    // falls back to `fg` in the build without a word. Aliases resolve in
    // document order, earlier declarations only (see
    // `render::ttf_builder::color::collect_color_aliases`); asking that same
    // collection which names made it into the map mirrors the build exactly.
    let color_aliases = crate::render::ttf_builder::collect_color_aliases(docs);
    for doc in docs {
        for (item_idx, item) in doc.items.iter().enumerate() {
            match item {
                DocumentItem::Color { name, value, .. } => {
                    if !color_aliases.contains_key(name) {
                        let why = if value.starts_with('#') {
                            format!("invalid color value `{value}`")
                        } else if color_aliases.contains_key(value) {
                            format!(
                                "`{value}` is declared later, and color aliases resolve \
                                     in document order"
                            )
                        } else {
                            format!("undeclared color `{value}`")
                        };
                        issues.push(issue_at(
                            doc,
                            item_idx,
                            Severity::Warning,
                            format!("color `{name}` never resolves: {why}"),
                        ));
                    }
                }
                DocumentItem::Glyph { body, .. } => {
                    for gref in &body.refs {
                        let Some(fill) = &gref.fill else { continue };
                        let c = &fill.color;
                        if c == "fg" {
                            continue;
                        }
                        if c.starts_with('#') {
                            if crate::render::ttf_builder::parse_hex_color(c).is_none() {
                                issues.push(issue_at(
                                    doc,
                                    item_idx,
                                    Severity::Warning,
                                    format!("invalid fill color `{c}`"),
                                ));
                            }
                        } else if !color_aliases.contains_key(c.as_str()) {
                            issues.push(issue_at(
                                doc,
                                item_idx,
                                Severity::Warning,
                                format!("fill names undeclared color `{c}`; it renders as `fg`"),
                            ));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}
