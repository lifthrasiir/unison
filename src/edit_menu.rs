#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EditAction {
    #[default]
    None,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    Delete,
    SelectAll,
}

pub struct EditMenuCaps {
    pub can_undo: bool,
    pub can_redo: bool,
    pub has_selection: bool,
    pub can_edit: bool,
}

/// Modifier-key prefixes for menu shortcut labels: `(command, shift)`.
pub fn platform_shortcut_names() -> (&'static str, &'static str) {
    if cfg!(target_os = "macos") {
        ("⌘", "⇧")
    } else {
        ("Ctrl+", "Shift+")
    }
}

impl EditAction {
    /// Every action an Edit menu lists, in its order. The `bool` is whether a
    /// separator goes *before* the entry.
    pub const MENU: [(EditAction, bool); 7] = [
        (EditAction::Undo, false),
        (EditAction::Redo, false),
        (EditAction::Cut, true),
        (EditAction::Copy, false),
        (EditAction::Paste, false),
        (EditAction::Delete, false),
        (EditAction::SelectAll, true),
    ];

    pub fn label(self) -> &'static str {
        match self {
            EditAction::None => "",
            EditAction::Undo => "Undo",
            EditAction::Redo => "Redo",
            EditAction::Cut => "Cut",
            EditAction::Copy => "Copy",
            EditAction::Paste => "Paste",
            EditAction::Delete => "Delete",
            EditAction::SelectAll => "Select All",
        }
    }

    pub fn shortcut(self) -> String {
        let (mod_name, shift_name) = platform_shortcut_names();
        match self {
            EditAction::None => String::new(),
            EditAction::Undo => format!("{mod_name}Z"),
            EditAction::Redo if cfg!(target_os = "macos") => format!("{mod_name}{shift_name}Z"),
            EditAction::Redo => format!("{mod_name}Y"),
            EditAction::Cut => format!("{mod_name}X"),
            EditAction::Copy => format!("{mod_name}C"),
            EditAction::Paste => format!("{mod_name}V"),
            EditAction::Delete => "Del".to_string(),
            EditAction::SelectAll => format!("{mod_name}A"),
        }
    }

    /// Whether the action does anything against `caps`.
    pub fn enabled(self, caps: &EditMenuCaps) -> bool {
        caps.can_edit
            && match self {
                EditAction::None => false,
                EditAction::Undo => caps.can_undo,
                EditAction::Redo => caps.can_redo,
                EditAction::Cut | EditAction::Copy | EditAction::Delete => caps.has_selection,
                EditAction::Paste | EditAction::SelectAll => true,
            }
    }
}

pub fn show_edit_menu_items(
    ui: &mut egui::Ui,
    caps: &EditMenuCaps,
    show_shortcuts: bool,
) -> EditAction {
    let mut action = EditAction::None;
    for (entry, separated) in EditAction::MENU {
        if separated {
            ui.separator();
        }
        let mut button = egui::Button::new(entry.label());
        if show_shortcuts {
            button = button.shortcut_text(entry.shortcut());
        }
        if ui.add_enabled(entry.enabled(caps), button).clicked() {
            action = entry;
            ui.close_menu();
        }
    }
    action
}
