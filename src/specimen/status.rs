//! What a laid-out cell shows: the status bar and tooltip text, the Ctrl/Cmd+C copy, and
//! the labels the cells carry.

use crate::glyph_flags::GlyphFlag;
use crate::ucd::variation_selector_label;

use super::{Item, SpecimenState, UvsEntry};

/// Tells a fresh Ctrl/Cmd+C from the auto-repeat of one still held down.
///
/// `egui-winit` turns every copy keystroke into a bare [`egui::Event::Copy`],
/// the repeats a held key emits included — and swallows the `Key::C` event that
/// would have carried the `repeat` flag, so nothing downstream can tell them
/// apart. Left alone, holding the shortcut copies the cell under the pointer
/// several times a second, and the character that ends up on the clipboard is
/// the one the pointer happened to rest on when the key came back *up*. Only
/// the first copy of a run counts.
#[derive(Default)]
pub(super) struct CopyKey {
    /// A copy has been made and nothing has ended the run yet.
    held: bool,
    /// When the last trigger arrived. macOS does not reliably deliver a key-up
    /// while the command modifier is down, so a run that never sees one has to
    /// end on its own; a repeat arrives far sooner than anyone presses the
    /// shortcut twice.
    last: Option<std::time::Instant>,
}

impl CopyKey {
    const REPEAT_GAP: std::time::Duration = std::time::Duration::from_millis(500);

    /// Whether this frame's `trigger` should copy. `key_up` is the run ending:
    /// C, or the modifier that made it a copy, seen going up.
    pub(super) fn accept(&mut self, trigger: bool, key_up: bool, now: std::time::Instant) -> bool {
        if key_up
            || self
                .last
                .is_some_and(|t| now.duration_since(t) > Self::REPEAT_GAP)
        {
            self.held = false;
        }
        if !trigger {
            return false;
        }
        self.last = Some(now);
        !std::mem::replace(&mut self.held, true)
    }
}

impl SpecimenState {
    /// What the report says about the glyph one cell draws, if anything.
    ///
    /// A cell whose character maps to no glyph at all is an error of its own:
    /// see [`CharEntry::unresolved`](super::CharEntry::unresolved) for why the flag map cannot carry it.
    pub(super) fn flag_for(&self, item: Item) -> Option<GlyphFlag> {
        let unresolved = match item {
            Item::Char(i) => self.entries[i].unresolved,
            Item::Uvs(i) => self.uvs_entries[i].unresolved,
            Item::Remap(_) => false,
        };
        if unresolved {
            return Some(GlyphFlag::Error);
        }
        self.glyph_flags.get(self.glyph_of(item)?)
    }

    /// The glyph a cell draws, `None` for a character the source declares
    /// nothing about.
    fn glyph_of(&self, item: Item) -> Option<&str> {
        match item {
            Item::Char(i) => self.entries[i].glyph_name.as_deref(),
            Item::Uvs(i) => Some(self.uvs_entries[i].glyph_name.as_str()),
            Item::Remap(ri) => Some(self.remap_entries[ri].glyph_name.as_str()),
        }
    }

    /// Where a click on this cell lands: the glyph whose own line carries the
    /// fault, if any, and otherwise the glyph the cell draws.
    ///
    /// A cell tinted because something it is *built out of* is broken is a cell
    /// whose own declaration is fine — for a Han character that declaration is
    /// a pattern line covering a whole block, which is not a place anyone
    /// wants to be sent. See [`crate::glyph_flags`]. For a glyph faulted
    /// directly the two names are the same and this changes nothing.
    pub(super) fn goto_target(&self, item: Item) -> Option<&str> {
        let name = self.glyph_of(item)?;
        Some(self.glyph_flags.source(name).unwrap_or(name))
    }

    /// The status-bar line for one cell, with what the report says about the
    /// glyph on the end of it. An inherited fault names the glyph it is really
    /// in, which is also where a click goes — see [`Self::goto_target`].
    pub(super) fn status_for(&self, item: Item) -> String {
        let mut line = self.status_body(item);
        if let Some(name) = self.glyph_of(item)
            && let Some(flag) = self.glyph_flags.get(name)
        {
            // Two words for two flags: a `Severity::Chore` is painted and named
            // as the warning it is, and the Issues tab is where the severity
            // that a build would not have printed is told apart from one it
            // would (see `glyph_flags`).
            let what = match flag {
                GlyphFlag::Warning => "warning",
                GlyphFlag::Error => "error",
            };
            line.push_str(&match self.glyph_flags.source(name) {
                Some(src) if src != name => format!(" \u{2014} {what} in '{src}'"),
                _ => format!(" \u{2014} {what}"),
            });
        }
        line
    }

    /// What Ctrl+C over one cell copies: the character, or the whole variation
    /// sequence — the two code points together are what a text field has to
    /// receive for the variant to show up in it.
    pub(super) fn copy_text(&self, item: Item) -> Option<String> {
        match item {
            Item::Char(i) => char::from_u32(self.entries[i].cp).map(|c| c.to_string()),
            Item::Uvs(i) => {
                let entry = &self.uvs_entries[i];
                let text: String = [entry.base, entry.selector]
                    .iter()
                    .filter_map(|cp| char::from_u32(*cp))
                    .collect();
                (text.chars().count() == 2).then_some(text)
            }
            Item::Remap(_) => None,
        }
    }

    pub(super) fn status_body(&self, item: Item) -> String {
        match item {
            Item::Char(i) => {
                let cp = self.entries[i].cp;
                let ch = char::from_u32(cp);
                let char_str = ch.map(|c| c.to_string()).unwrap_or_default();
                let char_name = self
                    .char_props
                    .name(cp)
                    .unwrap_or_else(|| "<unknown>".to_string());
                // Same brace group as the Ctrl+K popup, so one character reads
                // identically in either place.
                let props = ch
                    .map(|c| format!(" {}", self.char_props.property_summary(c)))
                    .unwrap_or_default();
                let tail = match &self.entries[i].glyph_name {
                    Some(name) => format!("({name})"),
                    None => "(undeclared)".to_string(),
                };
                format!("U+{cp:04X} {char_str} {char_name}{props} {tail}")
            }
            Item::Uvs(i) => {
                let entry = &self.uvs_entries[i];
                let base_name = self
                    .char_props
                    .name(entry.base)
                    .unwrap_or_else(|| "<unknown>".to_string());
                let text: String = [entry.base, entry.selector]
                    .iter()
                    .filter_map(|cp| char::from_u32(*cp))
                    .collect();
                format!(
                    "U+{:04X} U+{:04X} {} {} {} ({})",
                    entry.base,
                    entry.selector,
                    text,
                    base_name,
                    selector_suffix(&self.char_props, entry.selector),
                    entry.glyph_name
                )
            }
            Item::Remap(ri) => {
                let entry = &self.remap_entries[ri];
                match &entry.cp_sequence {
                    Some(cps) => {
                        let parts: Vec<String> = cps
                            .iter()
                            .map(|cp| {
                                let char_name = self
                                    .char_props
                                    .name(*cp)
                                    .unwrap_or_else(|| "<unknown>".to_string());
                                format!("U+{cp:04X} {char_name}")
                            })
                            .collect();
                        format!("{} ({})", parts.join(" + "), entry.glyph_name)
                    }
                    None => format!("{} (remap-only)", entry.glyph_name),
                }
            }
        }
    }
}

/// A variation-sequence cell's label: `+VS17`, the selector's short name alone.
/// A selector outside the two `VS` ranges (and the Mongolian ones) has no such
/// name and falls back to its code point.
///
/// The base is not repeated here: the cell sits right after the one it varies
/// and shares an open box with it, so its code point is already on
/// screen, and the short form is the one that fits a cell. Where there is no
/// such neighbour to read it against — the status bar, the tooltip — the pair
/// is spelled out in full instead (`status_body`).
///
/// A `prop … label` line replaces the whole token, leading `+` or `-` included
/// ([`CharProps::selector_label`](crate::ucd::CharProps::selector_label)): what
/// the source wrote is what the cell says, since the point of stating one is
/// that `+VS16` was not the name this font's reader wanted.
pub(super) fn uvs_label(char_props: &crate::ucd::CharProps, entry: &UvsEntry) -> String {
    match char_props.selector_label(entry.selector) {
        Some(label) => label.to_string(),
        None => format!("+{}", selector_name(entry.selector)),
    }
}

/// What `status_body` writes between the base's name and the glyph's:
/// `+ VS17`, the selector spelled apart from the `+` that says it is something
/// to type after the base.
///
/// A stated label is written whole and attached instead — `+EP`, `-EP` — since
/// its first character is part of what it says (`-EP` for the text
/// presentation selector against `+EP` for the emoji one), and prising a `+`
/// off the front would make the two read alike.
fn selector_suffix(char_props: &crate::ucd::CharProps, selector: u32) -> String {
    match char_props.selector_label(selector) {
        Some(label) => label.to_string(),
        None => format!("+ {}", selector_name(selector)),
    }
}

fn selector_name(selector: u32) -> String {
    variation_selector_label(selector).unwrap_or_else(|| format!("U+{selector:04X}"))
}

/// `12 / 128 (9.4%)` — how much of a block the source covers.
pub(super) fn format_coverage((declared, total): (usize, usize)) -> String {
    let pct = if total == 0 {
        0.0
    } else {
        declared as f32 * 100.0 / total as f32
    };
    format!("{declared} / {total} ({pct:.1}%)")
}
