//! Color literals, `color` aliases and layer-visibility resolution.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

/// `#RRGGBB` or `#RRGGBBAA`, hex digits and nothing else.
pub fn parse_hex_color(s: &str) -> Option<Rgba> {
    let s = s.strip_prefix('#')?;
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    let a = match s.len() {
        6 => 255,
        8 => byte(6)?,
        _ => return None,
    };
    Some(Rgba {
        r: byte(0)?,
        g: byte(2)?,
        b: byte(4)?,
        a,
    })
}

pub type ColorAliasMap = HashMap<String, (Rgba, Option<LayerVisibility>)>;

pub fn collect_color_aliases(docs: &[&Document]) -> ColorAliasMap {
    let mut map = ColorAliasMap::default();
    for doc in docs {
        for item in &doc.items {
            if let DocumentItem::Color {
                name,
                value,
                visibility,
                ..
            } = item
            {
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

pub fn resolve_fill_rgba(fill: &RefFill, color_aliases: &ColorAliasMap) -> Option<Rgba> {
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
    if let Some(fill) = fill
        && !fill.color.starts_with('#')
        && fill.color != "fg"
        && let Some((_, Some(vis))) = color_aliases.get(&fill.color)
    {
        return *vis;
    }
    LayerVisibility::Both
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors_are_exactly_hex_digits() {
        assert_eq!(
            parse_hex_color("#12abEF"),
            Some(Rgba {
                r: 0x12,
                g: 0xab,
                b: 0xef,
                a: 255
            })
        );
        assert_eq!(
            parse_hex_color("#12abef80"),
            Some(Rgba {
                r: 0x12,
                g: 0xab,
                b: 0xef,
                a: 0x80
            })
        );
        // Six *bytes* that are not six digits: no panic on a char boundary, and
        // no sign that `from_str_radix` would otherwise let through.
        assert_eq!(parse_hex_color("#aébcd"), None);
        assert_eq!(parse_hex_color("#éééé"), None);
        assert_eq!(parse_hex_color("#+1+2+3"), None);
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("123456"), None);
    }
}
