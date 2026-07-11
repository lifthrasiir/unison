#[derive(Clone, Copy, PartialEq)]
pub enum EditAction {
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

pub fn show_edit_menu_items(
    ui: &mut egui::Ui,
    caps: &EditMenuCaps,
    show_shortcuts: bool,
) -> EditAction {
    let mut action = EditAction::None;

    let (mod_name, shift_name) = if cfg!(target_os = "macos") {
        ("⌘", "⇧")
    } else {
        ("Ctrl+", "Shift+")
    };

    macro_rules! btn {
        ($label:expr) => {
            egui::Button::new($label)
        };
        ($label:expr, $shortcut:expr) => {
            if show_shortcuts {
                egui::Button::new($label).shortcut_text($shortcut)
            } else {
                egui::Button::new($label)
            }
        };
    }

    if ui
        .add_enabled(
            caps.can_undo && caps.can_edit,
            btn!("Undo", format!("{mod_name}Z")),
        )
        .clicked()
    {
        action = EditAction::Undo;
        ui.close_menu();
    }
    let redo_shortcut = if cfg!(target_os = "macos") {
        format!("{mod_name}{shift_name}Z")
    } else {
        format!("{mod_name}Y")
    };
    if ui
        .add_enabled(
            caps.can_redo && caps.can_edit,
            btn!("Redo", redo_shortcut),
        )
        .clicked()
    {
        action = EditAction::Redo;
        ui.close_menu();
    }

    ui.separator();

    if ui
        .add_enabled(
            caps.has_selection && caps.can_edit,
            btn!("Cut", format!("{mod_name}X")),
        )
        .clicked()
    {
        action = EditAction::Cut;
        ui.close_menu();
    }
    if ui
        .add_enabled(
            caps.has_selection && caps.can_edit,
            btn!("Copy", format!("{mod_name}C")),
        )
        .clicked()
    {
        action = EditAction::Copy;
        ui.close_menu();
    }
    if ui
        .add_enabled(caps.can_edit, btn!("Paste", format!("{mod_name}V")))
        .clicked()
    {
        action = EditAction::Paste;
        ui.close_menu();
    }
    if ui
        .add_enabled(
            caps.has_selection && caps.can_edit,
            btn!("Delete", format!("Del")),
        )
        .clicked()
    {
        action = EditAction::Delete;
        ui.close_menu();
    }

    ui.separator();

    if ui
        .add_enabled(caps.can_edit, btn!("Select All", format!("{mod_name}A")))
        .clicked()
    {
        action = EditAction::SelectAll;
        ui.close_menu();
    }

    action
}
