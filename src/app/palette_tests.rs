//! The palette's matching and its wiring into the frame.
//!
//! A child module of [`super`] (declared through `#[path]`), so it still
//! reaches private items.

use super::*;

fn items(
    files: &[&str],
    commands: &[&str],
    glyphs: &[&str],
    chars: &[(u32, &str)],
) -> PaletteItems {
    PaletteItems {
        files: files
            .iter()
            .map(|name| PaletteFile {
                name: name.to_string(),
                path: PathBuf::from(name),
            })
            .collect(),
        commands: commands
            .iter()
            .map(|title| PaletteCommand {
                command: Command::Save,
                title: title.to_string(),
                shortcut: String::new(),
            })
            .collect(),
        glyphs: glyphs
            .iter()
            .map(|g| g.to_string())
            .collect::<Vec<_>>()
            .into(),
        chars: chars
            .iter()
            .map(|&(cp, g)| PaletteChar {
                cp,
                selector: None,
                glyph: g.to_string(),
            })
            .collect::<Vec<_>>()
            .into(),
    }
}

/// The same, with a variation selector on every character.
fn items_with_sequences(chars: &[(u32, Option<u32>, &str)]) -> PaletteItems {
    let mut items = items(&[], &[], &[], &[]);
    items.chars = chars
        .iter()
        .map(|&(cp, selector, g)| PaletteChar {
            cp,
            selector,
            glyph: g.to_string(),
        })
        .collect::<Vec<_>>()
        .into();
    items
}

fn labels(items: &PaletteItems, rows: &[Row]) -> Vec<String> {
    rows.iter()
        .map(|row| match *row {
            Row::File(i) => items.files[i].name.clone(),
            Row::Command(i) => items.commands[i].title.clone(),
            Row::Glyph(i) => items.glyphs[i].clone(),
            Row::Char(i) => match items.chars[i].selector {
                None => format!("U+{:04X}", items.chars[i].cp),
                Some(sel) => format!("U+{:04X} U+{sel:04X}", items.chars[i].cp),
            },
        })
        .collect()
}

/// The query's letters have to appear in order and nothing more, case aside;
/// how closely they sit together only decides which group the row is in.
#[test]
fn a_query_matches_its_letters_in_order_whatever_the_case() {
    assert_eq!(text_fit("latin-a", "LAT"), Some(Fit::Prefix));
    assert_eq!(text_fit("latin-a", "tin"), Some(Fit::Substring));
    assert_eq!(text_fit("latin-a", "ltn"), Some(Fit::Subsequence));
    assert_eq!(text_fit("latin-aa", "aaa"), Some(Fit::Subsequence));
    assert_eq!(text_fit("latin-a", "nt"), None, "out of order");
    assert_eq!(text_fit("latin-a", "latin-ab"), None, "longer than the row");
    assert_eq!(text_fit("latin-a", ""), Some(Fit::Prefix));
    // A file name need not be ASCII.
    assert_eq!(text_fit("Ünicode.unf", "üNI"), Some(Fit::Prefix));
    assert_eq!(text_fit("Ünicode.unf", "ücd"), Some(Fit::Subsequence));
    assert_eq!(text_fit("Ünïcode.unf", "üni"), None, "ï is not i");
    assert_eq!(text_fit("abc", "ü"), None);
}

#[test]
fn a_code_point_query_is_u_plus_or_uni_then_hex_digits() {
    assert_eq!(codepoint_query("U+123"), Some("123"));
    assert_eq!(codepoint_query("u+"), Some(""));
    assert_eq!(codepoint_query("uni00AD"), Some("00AD"));
    assert_eq!(codepoint_query("UNIab"), Some("ab"));
    assert_eq!(codepoint_query("U+10FFFF"), Some("10FFFF"));
    assert_eq!(codepoint_query("U+1234567"), None, "seven digits");
    assert_eq!(codepoint_query("u+12g"), None);
    assert_eq!(codepoint_query("unicode"), None);
    assert_eq!(codepoint_query("u"), None);
    assert_eq!(codepoint_query("é+12"), None);
}

/// `U+123` is the start of U+123x and U+123xx, and the end of U+0123 — which is
/// the character most likely meant, so it is listed with the prefixes. Any
/// other code point it merely ends is listed after them.
#[test]
fn u_plus_123_lists_0123_and_every_code_point_it_starts() {
    let fit =
        |cp: u32, digits: &str| codepoint_fit(cp, digits, u32::from_str_radix(digits, 16).ok());
    assert_eq!(fit(0x123, "123"), Some(Fit::Prefix));
    assert_eq!(fit(0x1234, "123"), Some(Fit::Prefix));
    assert_eq!(fit(0x12345, "123"), Some(Fit::Prefix));
    assert_eq!(fit(0xA123, "123"), Some(Fit::Substring));
    assert_eq!(fit(0x1F123, "123"), Some(Fit::Substring));
    assert_eq!(fit(0x124, "123"), None);
    assert_eq!(fit(0x23, "123"), None);
    // Case, and more leading zeros than the written form has.
    assert_eq!(fit(0xABCD, "abc"), Some(Fit::Prefix));
    assert_eq!(fit(0x123, "000123"), Some(Fit::Prefix));
    assert_eq!(fit(0x1230, "000123"), None);
    // No digits yet: every code point.
    assert_eq!(fit(0x41, ""), Some(Fit::Prefix));
}

/// A prefix before a substring before a subsequence, whatever kind each row
/// is; within one group files, then menu entries, then glyphs.
#[test]
fn rows_are_grouped_by_fit_then_by_kind() {
    let items = items(
        &["glyphs.unf"],
        &["View: Show glyph metrics"],
        &["g-l-y-p-h", "glyph", "my-glyph"],
        &[(0x67, "g")],
    );
    assert_eq!(
        labels(&items, &narrow(&items, "glyph")),
        [
            "glyphs.unf",
            "glyph",
            "View: Show glyph metrics",
            "my-glyph",
            "g-l-y-p-h"
        ],
    );
    // Characters take no part in a query that is not a code point.
    assert!(!narrow(&items, "").contains(&Row::Char(0)));
}

/// A query spelled as a code point is asking for a character, so characters
/// come first; a glyph name that happens to match still follows.
#[test]
fn a_code_point_query_lists_characters_first() {
    let items = items(
        &[],
        &[],
        &["uni0123"],
        &[(0x41, "a"), (0x123, "x"), (0x1230, "y"), (0xA123, "z")],
    );
    assert_eq!(
        labels(&items, &narrow(&items, "uni123")),
        ["U+0123", "U+1230", "U+A123", "uni0123"],
    );
    assert_eq!(
        labels(&items, &narrow(&items, "U+")),
        ["U+0041", "U+0123", "U+1230", "U+A123"],
    );
}

/// One or two characters are read as the character itself — the rule `map`'s
/// own first token is read by — and anything longer is a name.
#[test]
fn a_literal_character_query_is_one_character_or_a_pair_with_a_selector() {
    assert_eq!(literal_char_query("\u{738B}"), Some((0x738B, None)));
    assert_eq!(literal_char_query("a"), Some((0x61, None)));
    assert_eq!(
        literal_char_query("\u{738B}\u{E0100}"),
        Some((0x738B, Some(0xE0100))),
    );
    assert_eq!(literal_char_query("0\u{FE0F}"), Some((0x30, Some(0xFE0F))));
    assert_eq!(literal_char_query(""), None);
    assert_eq!(literal_char_query("ab"), None, "no selector");
    assert_eq!(
        literal_char_query("\u{FE0F}\u{FE0F}"),
        None,
        "a selector is not a base",
    );
    assert_eq!(literal_char_query("\u{738B}\u{E0100}a"), None, "too long");
}

/// Typing the character asks for that character, and puts it first as a code
/// point query does; the sequences built on it come with it.
#[test]
fn a_literal_character_query_lists_that_character_first() {
    let items = items_with_sequences(&[
        (0x738A, None, "wang-ish"),
        (0x738B, None, "wang"),
        (0x738B, Some(0xE0100), "wang-alt"),
        (0x738C, None, "wang-other"),
    ]);
    assert_eq!(
        labels(&items, &narrow(&items, "\u{738B}")),
        ["U+738B", "U+738B U+E0100"],
    );
    assert_eq!(
        labels(&items, &narrow(&items, "\u{738B}\u{E0100}")),
        ["U+738B U+E0100"],
    );
    // The selector's own code point is not a character the font maps.
    assert!(narrow(&items, "\u{E0100}").is_empty());
}

/// A character the font does not map leaves the query an ordinary one, since
/// one or two characters is also what a short name looks like.
#[test]
fn a_literal_character_the_font_does_not_map_stays_an_ordinary_query() {
    let unmapped = items(&["a.unf"], &[], &["alpha"], &[(0x62, "b")]);
    assert_eq!(
        labels(&unmapped, &narrow(&unmapped, "a")),
        ["a.unf", "alpha"]
    );
    let mapped = items(&["a.unf"], &[], &["alpha"], &[(0x61, "a-glyph")]);
    assert_eq!(
        labels(&mapped, &narrow(&mapped, "a")),
        ["U+0061", "a.unf", "alpha"],
    );
}

/// What the listing is made of: the built font's cmap, the format 14 variation
/// sequences included. A "use default" sequence carries no glyph of its own and
/// is listed under the base's.
#[test]
fn the_listing_carries_the_fonts_variation_sequences() {
    let src = "\
meta height 4
meta ascent 3
meta descent 1

glyph zero 2 2
@@
@.

glyph zero-emoji 2 2
@@
@@

map U+0030 = zero
map U+0030 U+FE0E = zero
map U+0030 U+FE0F = zero-emoji
";
    let doc = crate::document_io::parse_document_from_str(src, "test.unf".into()).unwrap();
    let built = crate::render::ttf_builder::build_font_with_gid_map(&[&doc]).expect("it builds");
    let name_to_gid: HashMap<String, u16> = built
        .gid_to_name
        .iter()
        .map(|(&gid, name)| (name.clone(), gid))
        .collect();
    let chars = mapped_chars(&built.ttf, &name_to_gid);
    let listed: Vec<(u32, Option<u32>, &str)> = chars
        .iter()
        .map(|c| (c.cp, c.selector, c.glyph.as_str()))
        .collect();
    assert!(listed.contains(&(0x30, None, "zero")), "{listed:?}");
    assert!(listed.contains(&(0x30, Some(0xFE0E), "zero")), "{listed:?}");
    assert!(
        listed.contains(&(0x30, Some(0xFE0F), "zero-emoji")),
        "{listed:?}"
    );
}

/// Every entry has a title of its own, or two rows of the palette would read
/// the same and do different things.
#[test]
fn every_menu_entry_has_a_distinct_title() {
    let ctx = egui::Context::default();
    let app = UniformApp::with_settings(&ctx, super::settings::Settings::default(), None);
    let titles: Vec<String> = Command::all(&app).iter().map(|c| c.title(&app)).collect();
    let mut unique = titles.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), titles.len(), "{titles:#?}");
}

/// The palette driven the way a reader drives it, through the frame.
mod app_tests {
    use super::*;
    use crate::app::background::startup_tests::TempDir;
    use crate::app::settings::Settings;

    /// What stands in for the editor: a focusable widget drawn every frame,
    /// which is all the palette has to hand the keyboard back to.
    fn stand_in() -> egui::Id {
        egui::Id::new("palette-test-stand-in")
    }

    fn app_with(tag: &str) -> (TempDir, egui::Context, UniformApp) {
        let dir = TempDir::new(tag);
        std::fs::write(
            dir.0.join("a.unf"),
            "meta height 4\nmeta ascent 3\nmeta descent 1\n\n\
             glyph alpha 2 2\n@@\n.@\n\nglyph beta 2 2\n..\n@@\n\n\
             map A = alpha\nmap B = beta\n",
        )
        .unwrap();
        std::fs::write(dir.0.join("b.unf"), "glyph gamma\nref alpha 0 0\n").unwrap();
        let ctx = egui::Context::default();
        let app = UniformApp::with_settings(&ctx, Settings::default(), Some(dir.0.clone()));
        (dir, ctx, app)
    }

    /// Runs the background pipeline until the resolve and the font it came
    /// with have both landed.
    fn settle(app: &mut UniformApp, ctx: &egui::Context) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.named_glyphs_gen != app.font_build_gen || app.font_data.is_none() {
            app.pump_background_pipeline(ctx);
            assert!(
                std::time::Instant::now() < deadline,
                "the pipeline never delivered"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn cmd_p() -> egui::Event {
        key(egui::Key::P, egui::Modifiers::COMMAND)
    }

    fn text(s: &str) -> egui::Event {
        egui::Event::Text(s.to_string())
    }

    /// One frame: the palette's part of it, then the stand-in. Returns the
    /// jump the frame picked.
    fn frame(
        app: &mut UniformApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Option<PaletteJump> {
        let modifiers = events
            .iter()
            .find_map(|e| match e {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            modifiers,
            events,
            ..Default::default()
        });
        let mut menu = MenuActions::default();
        let jump = app.palette_frame(ctx, &mut menu);
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add(egui::TextEdit::singleline(&mut String::new()).id(stand_in()));
        });
        let _ = ctx.end_pass();
        jump
    }

    fn focused(ctx: &egui::Context) -> Option<egui::Id> {
        ctx.memory(|m| m.focused())
    }

    /// Gives the stand-in the keyboard, as an editor holding it would.
    fn focus_stand_in(app: &mut UniformApp, ctx: &egui::Context) {
        ctx.memory_mut(|m| m.request_focus(stand_in()));
        frame(app, ctx, vec![]);
        assert_eq!(focused(ctx), Some(stand_in()));
    }

    fn first_row(app: &UniformApp) -> String {
        let palette = app.palette.as_ref().expect("the palette is open");
        labels(&palette.items, &palette.rows[..1]).remove(0)
    }

    /// Ctrl/Cmd+P takes the keyboard into the palette's field, and Escape hands
    /// it back to where it was — which only the palette remembers.
    #[test]
    fn ctrl_p_takes_the_keyboard_and_escape_hands_it_back() {
        let (_dir, ctx, mut app) = app_with("palette-focus");
        focus_stand_in(&mut app, &ctx);

        frame(&mut app, &ctx, vec![cmd_p()]);
        assert!(app.palette.is_some());
        frame(&mut app, &ctx, vec![]);
        assert_eq!(focused(&ctx), Some(field_id()));

        frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Escape, Default::default())],
        );
        assert!(app.palette.is_none());
        assert_eq!(focused(&ctx), Some(stand_in()));
    }

    /// The list keys walk the rows instead of reaching the field, completion's
    /// aliases included; Left and Right are the field's.
    #[test]
    fn the_list_keys_walk_the_rows() {
        let (_dir, ctx, mut app) = app_with("palette-walk");
        focus_stand_in(&mut app, &ctx);
        frame(&mut app, &ctx, vec![cmd_p()]);
        // The frame the opening asks for: the field claims the list keys from
        // egui only once it has held the focus for one, and without it the
        // first arrow below moves the focus out of the palette instead.
        frame(&mut app, &ctx, vec![]);
        let selected = |app: &UniformApp| app.palette.as_ref().unwrap().nav.selected;
        let none = egui::Modifiers::NONE;

        frame(&mut app, &ctx, vec![key(egui::Key::ArrowDown, none)]);
        assert_eq!(selected(&app), 1);
        frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::J, egui::Modifiers::CTRL)],
        );
        assert_eq!(selected(&app), 2);
        frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::K, egui::Modifiers::CTRL)],
        );
        assert_eq!(selected(&app), 1);
        frame(&mut app, &ctx, vec![key(egui::Key::ArrowLeft, none)]);
        assert_eq!(selected(&app), 1);
        frame(&mut app, &ctx, vec![key(egui::Key::End, none)]);
        let rows = app.palette.as_ref().unwrap().rows.len();
        assert_eq!(selected(&app), rows - 1);
        assert_eq!(app.palette.as_ref().unwrap().query, "");
        assert_eq!(
            focused(&ctx),
            Some(field_id()),
            "and the field keeps the keyboard"
        );
    }

    /// A menu entry that could do nothing now is not offered at all, and the
    /// palette does not offer itself.
    #[test]
    fn a_disabled_menu_entry_is_not_offered() {
        let (_dir, ctx, mut app) = app_with("palette-disabled");
        frame(&mut app, &ctx, vec![cmd_p()]);
        let palette = app.palette.as_ref().unwrap();
        let titles: Vec<&str> = palette
            .items
            .commands
            .iter()
            .map(|c| c.title.as_str())
            .collect();
        assert!(titles.contains(&"View: Show glyph metrics"), "{titles:?}");
        assert!(
            !titles.contains(&"Edit: Go back"),
            "no history to go back through"
        );
        assert!(!titles.iter().any(|t| t.contains("Command palette")));
    }

    /// A command runs once the keyboard is back where the palette took it
    /// from, which is the frame after it was picked.
    #[test]
    fn a_picked_command_runs_on_the_next_frame() {
        let (_dir, ctx, mut app) = app_with("palette-command");
        focus_stand_in(&mut app, &ctx);
        frame(&mut app, &ctx, vec![cmd_p()]);
        frame(&mut app, &ctx, vec![text("glyph metrics")]);
        assert_eq!(first_row(&app), "View: Show glyph metrics");

        let before = app.show_metrics;
        frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Tab, Default::default())],
        );
        assert!(app.palette.is_none());
        assert_eq!(app.show_metrics, before, "not on the frame it was picked");
        assert_eq!(focused(&ctx), Some(stand_in()));

        frame(&mut app, &ctx, vec![]);
        assert_eq!(app.show_metrics, !before);
        assert_eq!(app.palette_command, None);
    }

    /// A glyph row goes to the glyph, and the jump is recorded from the caret
    /// it left, as a search hit's is.
    #[test]
    fn a_glyph_row_goes_to_the_glyph_and_records_the_jump() {
        let (dir, ctx, mut app) = app_with("palette-glyph");
        settle(&mut app, &ctx);
        app.open_file(dir.0.join("b.unf"));

        frame(&mut app, &ctx, vec![cmd_p()]);
        frame(&mut app, &ctx, vec![text("bet")]);
        assert_eq!(first_row(&app), "beta");
        let jump = frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Enter, Default::default())],
        );
        app.apply_palette_jump(&ctx, jump.expect("Enter takes the row"));

        assert_caret_on(&app, "a.unf", "glyph beta 2 2");
        assert!(app.nav_history.can_go_back());
    }

    /// `U+42` lists the character the font maps there, and taking it goes to
    /// the glyph it maps to.
    #[test]
    fn a_code_point_row_goes_to_the_glyph_it_maps_to() {
        let (dir, ctx, mut app) = app_with("palette-codepoint");
        settle(&mut app, &ctx);
        app.open_file(dir.0.join("b.unf"));

        frame(&mut app, &ctx, vec![cmd_p()]);
        frame(&mut app, &ctx, vec![text("u+42")]);
        assert_eq!(first_row(&app), "U+0042");
        let jump = frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Enter, Default::default())],
        );
        app.apply_palette_jump(&ctx, jump.expect("Enter takes the row"));

        assert_caret_on(&app, "a.unf", "glyph beta 2 2");
    }

    /// A file row opens the file.
    #[test]
    fn a_file_row_opens_the_file() {
        let (_dir, ctx, mut app) = app_with("palette-file");
        frame(&mut app, &ctx, vec![cmd_p()]);
        frame(&mut app, &ctx, vec![text("b.unf")]);
        assert_eq!(first_row(&app), "b.unf");
        let jump = frame(
            &mut app,
            &ctx,
            vec![key(egui::Key::Enter, Default::default())],
        );
        app.apply_palette_jump(&ctx, jump.expect("Enter takes the row"));
        let idx = app.panes.active_doc_idx().expect("a document is showing");
        assert!(app.open_documents[idx].document.path.ends_with("b.unf"));
    }

    fn assert_caret_on(app: &UniformApp, file: &str, line: &str) {
        let idx = app.panes.active_doc_idx().expect("a document is showing");
        let doc = &app.open_documents[idx];
        assert!(doc.document.path.ends_with(file), "{:?}", doc.document.path);
        let at = doc.editor_state.cursor_line();
        assert_eq!(doc.lines[at].as_text(), Some(line));
    }
}
