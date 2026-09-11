//! The palette (Ctrl/Cmd+P): one box that opens a file, goes to a glyph or to
//! the glyph a character maps to, or runs a menu entry.
//!
//! # What it lists
//!
//! The files the sidebar lists; every menu entry that is enabled when the
//! palette opens, as its [`Command`]; the resolved glyph names completion would
//! offer ([`crate::editor::autocomplete::offers_glyph_name`]); and every
//! character the built font's cmap maps. The last two are a whole font's worth
//! of rows, so they are kept in [`PaletteCache`] and rebuilt only when a
//! rebuild replaced what they were made from — out of memory, never out of a
//! file, since this runs on the UI thread.
//!
//! # How a query matches
//!
//! Everything but a character matches by **subsequence**: the query's
//! characters appear in the row's text in order, anything in between, case
//! ignored. The rows are then grouped by how well they matched ([`Fit`]) — a
//! prefix before a substring before a scattered subsequence — and within that by
//! kind, each kind in its own order. The grouping is a bucket fill, not a sort:
//! a query is narrowed over a font's worth of glyph names on every keystroke.
//!
//! A character is matched only by a query spelled like a code point — `U+` or
//! `uni` and up to six hex digits ([`codepoint_query`]) — and then the digits
//! are matched against the code point written with at least four of them, as a
//! prefix *or* a suffix ([`codepoint_fit`]). The suffix is what makes `U+123`
//! list U+0123 alongside U+123x and U+123xx. Such a query puts characters first.
//!
//! # The keyboard
//!
//! The walk and its keys are [`crate::editor::list_popup`]'s typed-list keys,
//! the ones completion answers to: the arrows, Ctrl+J/K, Home/End, PgUp/PgDn,
//! Enter or Tab to take a row, Escape to give up. Left and Right stay the
//! field's own. The field is an `egui::TextEdit`, which would act on every one
//! of those keys itself, so they are read and then taken out of the queue
//! ([`take_list_keys`]), at the top of the frame before anything else reads a
//! key. Two of them egui reads earlier still, in `begin_pass`: Tab moves the
//! focus on unless the focused widget claims it, which the field does
//! (`lock_focus`, harmless on one line), and Escape drops the focus outright —
//! which is why dismissing hands it back explicitly.
//!
//! # Where the keyboard goes back to
//!
//! Opening records the widget that held the keyboard ([`PaletteState`]'s
//! `return_focus`), separately from anything else: the palette's field takes the
//! focus away from it, and nothing else remembers where it was. Escape, or a
//! click outside the palette onto nothing that takes the focus itself, hands the
//! keyboard back there. A jump gives it to the editor the jump lands in
//! instead. A command hands it back and then runs on the **next** frame: the
//! command acts on whatever had the keyboard — the enabled tests, the Edit
//! entries' target and the zoom target all read it — and that surface needs one
//! frame of holding the focus again before it says so.

use std::path::PathBuf;
use std::sync::Arc;

use super::commands::{Command, CommandCx};
use super::menus::{EditTarget, MenuActions};
use super::*;
use crate::editor::codepoint_popup::{FieldFrame, FieldOutcome, resolve_field, restore_host_focus};
use crate::editor::list_popup::{
    ListMove, ListNav, TypedListKey, read_typed_list_key, show_window,
};
use crate::hash::HashSet;

/// The field's id. There is one palette per window, so a fixed id is right.
pub(super) fn field_id() -> egui::Id {
    egui::Id::new("uniform_palette_query")
}

/// How well a row matched: the order the palette lists its groups in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Fit {
    /// The query starts the row. For a code point: the digits start it, or
    /// name exactly it.
    Prefix,
    /// The query appears in the row unbroken. For a code point: the digits end
    /// it.
    Substring,
    /// Only the query's characters appear, in order.
    Subsequence,
}

/// How `label` matches `query`, ignoring case; `None` when the query's
/// characters do not all appear in it in order. An empty query matches
/// everything as a prefix.
pub(super) fn text_fit(label: &str, query: &str) -> Option<Fit> {
    if query.is_empty() {
        return Some(Fit::Prefix);
    }
    // Every glyph name is ASCII, and they are nearly every row.
    if label.is_ascii() && query.is_ascii() {
        return ascii_fit(label.as_bytes(), query.as_bytes());
    }
    let (label, query) = (label.to_lowercase(), query.to_lowercase());
    let mut want = query.chars().peekable();
    for c in label.chars() {
        if want.next_if_eq(&c).is_some() && want.peek().is_none() {
            break;
        }
    }
    if want.peek().is_some() {
        return None;
    }
    Some(if label.starts_with(&query) {
        Fit::Prefix
    } else if label.contains(&query) {
        Fit::Substring
    } else {
        Fit::Subsequence
    })
}

fn ascii_fit(label: &[u8], query: &[u8]) -> Option<Fit> {
    let mut want = query.iter().peekable();
    for b in label {
        if want.next_if(|q| q.eq_ignore_ascii_case(b)).is_some() && want.peek().is_none() {
            break;
        }
    }
    if want.peek().is_some() {
        return None;
    }
    // A subsequence match means the label is at least as long as the query.
    Some(if label[..query.len()].eq_ignore_ascii_case(query) {
        Fit::Prefix
    } else if label
        .windows(query.len())
        .any(|w| w.eq_ignore_ascii_case(query))
    {
        Fit::Substring
    } else {
        Fit::Subsequence
    })
}

/// The hex digits of a query spelled like a code point — `U+` or `uni`, in any
/// case, then at most six hex digits — or `None` for any other query. The
/// digits may be none at all: `U+` alone is the start of every code point.
pub(super) fn codepoint_query(query: &str) -> Option<&str> {
    let strip = |prefix: &str| {
        let head = query.get(..prefix.len())?;
        head.eq_ignore_ascii_case(prefix)
            .then(|| &query[prefix.len()..])
    };
    let digits = strip("u+").or_else(|| strip("uni"))?;
    (digits.len() <= 6 && digits.bytes().all(|b| b.is_ascii_hexdigit())).then_some(digits)
}

/// How the typed `digits` match code point `cp`, written with at least four
/// digits: a prefix (or the very same value) is [`Fit::Prefix`], a suffix is
/// [`Fit::Substring`]. `value` is the digits parsed, passed in so that a whole
/// cmap is not parsed against once per row.
pub(super) fn codepoint_fit(cp: u32, digits: &str, value: Option<u32>) -> Option<Fit> {
    let mut buf = [0u8; 8];
    let written = hex_digits(cp, &mut buf);
    let typed = digits.as_bytes();
    if written.len() < typed.len() {
        return (value == Some(cp)).then_some(Fit::Prefix);
    }
    if value == Some(cp) || written[..typed.len()].eq_ignore_ascii_case(typed) {
        Some(Fit::Prefix)
    } else if written[written.len() - typed.len()..].eq_ignore_ascii_case(typed) {
        Some(Fit::Substring)
    } else {
        None
    }
}

/// `cp` in upper-case hex, at least four digits, as `U+` writes it.
fn hex_digits(cp: u32, buf: &mut [u8; 8]) -> &[u8] {
    const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
    let (mut v, mut n) = (cp, 0);
    while n < 4 || v != 0 {
        buf[7 - n] = DIGITS[(v & 0xF) as usize];
        v >>= 4;
        n += 1;
    }
    &buf[8 - n..]
}

pub(super) struct PaletteFile {
    pub name: String,
    pub path: PathBuf,
}

pub(super) struct PaletteCommand {
    pub command: Command,
    /// `Menu: Entry`, what is listed and matched.
    pub title: String,
    pub shortcut: String,
}

/// Everything one opening of the palette can list.
pub(super) struct PaletteItems {
    pub files: Vec<PaletteFile>,
    pub commands: Vec<PaletteCommand>,
    /// Sorted.
    pub glyphs: Arc<[String]>,
    /// `(code point, glyph name)`, by code point.
    pub chars: Arc<[(u32, String)]>,
}

/// One listed row, by index into its kind's list in [`PaletteItems`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row {
    File(usize),
    Command(usize),
    Glyph(usize),
    Char(usize),
}

/// The rows `query` leaves, in the order they are listed; see the module note.
pub(super) fn narrow(items: &PaletteItems, query: &str) -> Vec<Row> {
    const KINDS: usize = 4;
    let codepoint = codepoint_query(query);
    // Kinds are numbered files, commands, glyphs, characters; a code point
    // query rotates characters to the front.
    let rank = |kind: usize| match codepoint {
        Some(_) => (kind + 1) % KINDS,
        None => kind,
    };
    let mut buckets: [Vec<Row>; 3 * KINDS] = Default::default();
    let mut put = |fit: Fit, kind: usize, row: Row| {
        buckets[fit as usize * KINDS + rank(kind)].push(row);
    };
    for (i, file) in items.files.iter().enumerate() {
        if let Some(fit) = text_fit(&file.name, query) {
            put(fit, 0, Row::File(i));
        }
    }
    for (i, command) in items.commands.iter().enumerate() {
        if let Some(fit) = text_fit(&command.title, query) {
            put(fit, 1, Row::Command(i));
        }
    }
    for (i, name) in items.glyphs.iter().enumerate() {
        if let Some(fit) = text_fit(name, query) {
            put(fit, 2, Row::Glyph(i));
        }
    }
    if let Some(digits) = codepoint {
        let value = u32::from_str_radix(digits, 16).ok();
        for (i, &(cp, _)) in items.chars.iter().enumerate() {
            if let Some(fit) = codepoint_fit(cp, digits, value) {
                put(fit, 3, Row::Char(i));
            }
        }
    }
    buckets.into_iter().flatten().collect()
}

/// Every character `font`'s cmap maps, with the name of the glyph it maps to,
/// by code point. `name_to_gid` is the build's own table; where several names
/// share a glyph (an alias), the smallest is taken, so the listing is the same
/// on every build.
pub(super) fn mapped_chars(font: &[u8], name_to_gid: &HashMap<String, u16>) -> Vec<(u32, String)> {
    use skrifa::MetadataProvider;
    let Ok(font) = skrifa::FontRef::new(font) else {
        return Vec::new();
    };
    let mut names: Vec<Option<&str>> = Vec::new();
    for (name, &gid) in name_to_gid {
        let gid = gid as usize;
        if names.len() <= gid {
            names.resize(gid + 1, None);
        }
        if names[gid].is_none_or(|n| name.as_str() < n) {
            names[gid] = Some(name);
        }
    }
    let mut chars: Vec<(u32, String)> = font
        .charmap()
        .mappings()
        .filter_map(|(cp, gid)| {
            let name = names.get(gid.to_u32() as usize).copied().flatten()?;
            Some((cp, name.to_string()))
        })
        .collect();
    chars.sort_unstable_by_key(|&(cp, _)| cp);
    chars.dedup_by_key(|(cp, _)| *cp);
    chars
}

/// The open palette.
pub(super) struct PaletteState {
    query: String,
    items: PaletteItems,
    rows: Vec<Row>,
    nav: ListNav,
    /// The query `rows` were narrowed by, so a frame that typed nothing leaves
    /// the walk where it is.
    narrowed_for: String,
    /// The widget that held the keyboard when the palette opened; see the
    /// module note.
    return_focus: Option<egui::Id>,
    focus_set: bool,
}

/// A row that was taken and is a jump, carried out after the frame's editors
/// have run as every other jump is. A command is not one of these: it waits a
/// frame instead ([`UniformApp::palette_command`]).
pub(super) enum PaletteJump {
    File(PathBuf),
    Glyph(String),
}

/// `(code point, glyph name)` for every mapped character, by code point.
type MappedChars = Arc<[(u32, String)]>;

/// The palette's big listings between openings, each with the generation it
/// was made from.
#[derive(Default)]
pub(super) struct PaletteCache {
    /// Keyed on `derived_gen`, which moves whenever `named_glyphs` is replaced.
    glyphs: Option<(u64, Arc<[String]>)>,
    /// Keyed on `font_data_gen`, which moves whenever the built font is.
    chars: Option<(u64, MappedChars)>,
}

/// Takes the keys the list walks with out of the queue. Left in, the field
/// would act on them: Enter surrenders its focus, the arrows and Home/End move
/// its cursor.
fn take_list_keys(ctx: &egui::Context) {
    use egui::Key::*;
    ctx.input_mut(|i| {
        i.events.retain(|e| {
            !matches!(
                e,
                egui::Event::Key {
                    key: Escape
                        | Enter
                        | Tab
                        | ArrowUp
                        | ArrowDown
                        | Home
                        | End
                        | PageUp
                        | PageDown
                        | J
                        | K,
                    pressed: true,
                    ..
                }
            )
        });
    });
}

/// One row: its kind letter and text on the left, what it adds on the right.
fn palette_row(
    ui: &mut egui::Ui,
    items: &PaletteItems,
    row: Row,
    selected: bool,
) -> egui::Response {
    let (kind, label, detail): (&str, String, String) = match row {
        Row::File(i) => ("F", items.files[i].name.clone(), String::new()),
        Row::Command(i) => {
            let command = &items.commands[i];
            (">", command.title.clone(), command.shortcut.clone())
        }
        Row::Glyph(i) => ("G", items.glyphs[i].clone(), String::new()),
        Row::Char(i) => {
            let (cp, glyph) = &items.chars[i];
            let shown = char::from_u32(*cp)
                .filter(|c| !c.is_control())
                .map_or_else(String::new, String::from);
            ("U", format!("U+{cp:04X}"), format!("{shown}  {glyph}"))
        }
    };
    let height = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let visuals = ui.visuals();
    if selected {
        ui.painter()
            .rect_filled(rect, 2.0, visuals.selection.bg_fill);
    } else if response.hovered() {
        ui.painter()
            .rect_filled(rect, 2.0, visuals.widgets.hovered.weak_bg_fill);
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let color = if selected {
        visuals.selection.stroke.color
    } else {
        visuals.text_color()
    };
    let pad = egui::vec2(4.0, 0.0);
    ui.painter().text(
        rect.left_center() + pad,
        egui::Align2::LEFT_CENTER,
        format!("{kind}  {label}"),
        font.clone(),
        color,
    );
    if !detail.is_empty() {
        ui.painter().text(
            rect.right_center() - pad,
            egui::Align2::RIGHT_CENTER,
            detail,
            font,
            visuals.weak_text_color(),
        );
    }
    response
}

impl UniformApp {
    /// The palette's part of a frame, run at the top of it: the command picked
    /// on the last frame, the chord that opens the palette, its keys, and the
    /// palette itself. Returns a jump that was picked, for after the editors.
    pub(super) fn palette_frame(
        &mut self,
        ctx: &egui::Context,
        menu: &mut MenuActions,
    ) -> Option<PaletteJump> {
        if let Some(command) = self.palette_command.take() {
            command.apply(self, ctx, menu);
        }

        // Shift is not ruled out: `consume_key` ignores it, and Ctrl/Cmd+Shift+P
        // is where a reader used to a separate command palette will look.
        let chord = ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::P));
        let from_menu = std::mem::take(&mut self.palette_requested);
        if self.palette.is_none() && (chord || from_menu) {
            // A chord leaves the focus where the reader had it. A menu entry
            // has already moved it onto the menu's own button, which goes away
            // with the menu, so the place to go back to is the editor the menu
            // would have acted on.
            let back = if from_menu {
                self.active_doc().and_then(|d| d.editor_state.canvas_id)
            } else {
                ctx.memory(|m| m.focused())
            };
            self.open_palette(back);
            // The field claims the list keys from egui only once it has held
            // the focus for a whole frame (`Memory::set_focus_lock_filter`), and
            // until then egui reads an arrow or a Tab as a move of the focus out
            // of it. So that frame is asked for now, rather than left to be the
            // one the next keystroke arrives in.
            ctx.request_repaint();
        }
        let palette = self.palette.as_mut()?;

        let (selected, len) = (palette.nav.selected, palette.rows.len());
        match ctx.input(|i| read_typed_list_key(i, selected, len)) {
            None | Some(TypedListKey::Move(ListMove::Sideways)) => {}
            Some(key) => {
                take_list_keys(ctx);
                match key {
                    TypedListKey::Dismiss => {
                        self.close_palette(ctx);
                        return None;
                    }
                    TypedListKey::Move(step) => palette.nav.step(step, len),
                    // Nothing to take keeps the palette open on the query that
                    // found nothing, for it to be corrected.
                    TypedListKey::Accept if len == 0 => {}
                    TypedListKey::Accept => return self.accept_palette(ctx, selected),
                }
            }
        }

        let screen = ctx.screen_rect();
        let width = (screen.width() * 0.6).clamp(240.0, 640.0);
        let shown = egui::Area::new(egui::Id::new("uniform_palette"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 40.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .show(ui, |ui| {
                        ui.set_width(width);
                        let field = ui.add(
                            egui::TextEdit::singleline(&mut palette.query)
                                .id(field_id())
                                // Tab is the palette's accept key, not a focus
                                // move; see the module note.
                                .lock_focus(true)
                                .desired_width(f32::INFINITY)
                                .hint_text("File, glyph, U+code point or menu entry"),
                        );
                        if !palette.focus_set {
                            field.request_focus();
                            palette.focus_set = true;
                        }
                        if palette.query != palette.narrowed_for {
                            palette.rows = narrow(&palette.items, &palette.query);
                            palette.nav = ListNav::new(0, palette.rows.len());
                            palette.narrowed_for.clone_from(&palette.query);
                        }
                        let clicked = if palette.rows.is_empty() {
                            ui.weak("No match");
                            None
                        } else {
                            let (items, rows) = (&palette.items, &palette.rows);
                            show_window(ui, &palette.nav, rows.len(), |ui, i, selected| {
                                palette_row(ui, items, rows[i], selected)
                            })
                        };
                        let frame = FieldFrame {
                            id: field.id,
                            lost_focus: field.lost_focus(),
                            confirmed: false,
                        };
                        (frame, clicked)
                    })
                    .inner
            });

        let (frame, clicked) = shown.inner;
        if let Some(i) = clicked {
            return self.accept_palette(ctx, i);
        }
        if let FieldOutcome::Cancel = resolve_field(ctx, &frame, shown.response.rect) {
            self.close_palette(ctx);
        }
        None
    }

    /// Opens the palette on an empty query, remembering `return_focus`.
    fn open_palette(&mut self, return_focus: Option<egui::Id>) {
        let cx = CommandCx {
            edit_target: if self.shaped_preview.is_focused() {
                EditTarget::Preview
            } else {
                EditTarget::Editor
            },
            editor_focused: self
                .active_doc()
                .is_some_and(|d| d.editor_state.is_active()),
        };
        let items = self.palette_items(cx);
        let rows = narrow(&items, "");
        self.palette = Some(PaletteState {
            query: String::new(),
            nav: ListNav::new(0, rows.len()),
            items,
            rows,
            narrowed_for: String::new(),
            return_focus,
            focus_set: false,
        });
    }

    /// Closes the palette without taking anything, handing the keyboard back
    /// unless something else has already taken it.
    fn close_palette(&mut self, ctx: &egui::Context) {
        if let Some(palette) = self.palette.take()
            && let Some(back) = palette.return_focus
        {
            restore_host_focus(ctx, back);
        }
    }

    /// Takes row `index` and closes the palette.
    fn accept_palette(&mut self, ctx: &egui::Context, index: usize) -> Option<PaletteJump> {
        let palette = self.palette.take()?;
        let row = *palette.rows.get(index)?;
        let jump = match row {
            Row::Command(i) => {
                match palette.return_focus {
                    Some(back) => ctx.memory_mut(|m| m.request_focus(back)),
                    None => ctx.memory_mut(|m| m.surrender_focus(field_id())),
                }
                self.palette_command = Some(palette.items.commands[i].command);
                ctx.request_repaint();
                return None;
            }
            Row::File(i) => PaletteJump::File(palette.items.files[i].path.clone()),
            Row::Glyph(i) => PaletteJump::Glyph(palette.items.glyphs[i].clone()),
            Row::Char(i) => PaletteJump::Glyph(palette.items.chars[i].1.clone()),
        };
        // The jump gives the keyboard to the editor it lands in; until then the
        // field it was in is gone.
        ctx.memory_mut(|m| m.surrender_focus(field_id()));
        Some(jump)
    }

    /// Carries out a jump the palette picked. A glyph is recorded in the
    /// history from the caret, as a search hit is: the palette is not a
    /// position in a document.
    pub(super) fn apply_palette_jump(&mut self, ctx: &egui::Context, jump: PaletteJump) {
        match jump {
            PaletteJump::File(path) => {
                self.open_file(path);
            }
            PaletteJump::Glyph(name) => {
                let from = self.caret_nav_loc();
                if !self.jump_to_name(ctx, &name, LinkTargetKind::Glyph, from) {
                    self.set_status(format!("Nothing declares {name}"));
                }
            }
        }
        self.refocus_active_editor();
        self.focus_pane_editor(ctx);
    }

    /// What this opening lists. The enabled test is `cx`'s, as of the opening.
    fn palette_items(&mut self, cx: CommandCx) -> PaletteItems {
        let files = self
            .sidebar
            .files()
            .iter()
            .map(|path| PaletteFile {
                name: docs::file_name_of(path),
                path: path.clone(),
            })
            .collect();
        let commands = Command::all(self)
            .into_iter()
            .filter(|&c| c != Command::Palette && c.enabled(self, cx))
            .map(|command| PaletteCommand {
                command,
                title: command.title(self),
                shortcut: command.shortcut(self),
            })
            .collect();
        PaletteItems {
            files,
            commands,
            glyphs: self.palette_glyph_names(),
            chars: self.palette_mapped_chars(),
        }
    }

    fn palette_glyph_names(&mut self) -> Arc<[String]> {
        if let Some((generation, names)) = &self.palette_cache.glyphs
            && *generation == self.derived_gen
        {
            return Arc::clone(names);
        }
        let mut declared = HashSet::default();
        for doc in self.collect_all_docs() {
            crate::editor::autocomplete::declared_glyph_names(doc, &mut declared);
        }
        let mut names: Vec<String> = self
            .named_glyphs
            .keys()
            .filter(|name| crate::editor::autocomplete::offers_glyph_name(name, &declared))
            .cloned()
            .collect();
        names.sort_unstable();
        let names: Arc<[String]> = names.into();
        self.palette_cache.glyphs = Some((self.derived_gen, Arc::clone(&names)));
        names
    }

    fn palette_mapped_chars(&mut self) -> Arc<[(u32, String)]> {
        if let Some((generation, chars)) = &self.palette_cache.chars
            && *generation == self.font_data_gen
        {
            return Arc::clone(chars);
        }
        let chars: Arc<[(u32, String)]> = self
            .font_data
            .as_ref()
            .map(|(_, vector)| mapped_chars(vector, &self.font_name_to_gid))
            .unwrap_or_default()
            .into();
        self.palette_cache.chars = Some((self.font_data_gen, Arc::clone(&chars)));
        chars
    }
}

#[cfg(test)]
#[path = "palette_tests.rs"]
mod palette_tests;
