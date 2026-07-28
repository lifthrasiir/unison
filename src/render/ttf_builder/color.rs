//! Color literals, `color` aliases and layer-visibility resolution.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

pub fn parse_hex_color(s: &str) -> Option<Rgba> {
    let s = s.strip_prefix('#')?;
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some(Rgba { r, g, b, a: 255 })
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some(Rgba { r, g, b, a })
        }
        _ => None,
    }
}

pub type ColorAliasMap = HashMap<String, (Rgba, Option<LayerVisibility>)>;

pub fn collect_color_aliases(docs: &[&Document]) -> ColorAliasMap {
    let mut map = ColorAliasMap::new();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Color { name, value, visibility, .. } = item {
                let resolved = resolve_color_value(value, &map);
                if let Some(rgba) = resolved {
                    map.insert(name.clone(), (rgba, *visibility));
                }
            }
        }
    }
    map
}

fn resolve_color_value(value: &str, aliases: &ColorAliasMap) -> Option<Rgba> {
    if value.starts_with('#') {
        parse_hex_color(value)
    } else if let Some((rgba, _)) = aliases.get(value) {
        Some(rgba.clone())
    } else {
        None
    }
}

pub fn resolve_fill_rgba(
    fill: &RefFill,
    color_aliases: &ColorAliasMap,
) -> Option<Rgba> {
    if fill.color == "fg" {
        return None;
    }
    if fill.color.starts_with('#') {
        return parse_hex_color(&fill.color);
    }
    color_aliases.get(&fill.color).map(|(rgba, _)| rgba.clone())
}

pub fn effective_visibility(
    ref_visibility: Option<LayerVisibility>,
    fill: Option<&RefFill>,
    color_aliases: &ColorAliasMap,
) -> LayerVisibility {
    if let Some(vis) = ref_visibility {
        return vis;
    }
    if let Some(fill) = fill {
        if !fill.color.starts_with('#') && fill.color != "fg"
            && let Some((_, Some(vis))) = color_aliases.get(&fill.color) {
                return *vis;
            }
    }
    LayerVisibility::Both
}
