use egui::Color32;

#[derive(Clone)]
pub struct Palette {
    // Grid
    pub grid_bg: Color32,
    pub grid_off: Color32,
    pub grid_on: Color32,
    pub grid_ext_off: Color32,
    pub grid_ext_grid: Color32,

    // Gutter
    pub line_num: Color32,

    // Syntax
    pub text_default: Color32,
    pub text_comment: Color32,
    pub text_heading: Color32,
    pub text_meta: Color32,
    pub text_header: Color32,
    pub text_ref: Color32,
    pub text_directive: Color32,
    pub text_directive2: Color32,

    // Errors & links
    pub error: Color32,
    pub link: Color32,

    // Selection & cursor
    pub selection: Color32,
    pub cursor_border: Color32,
    pub grid_border: Color32,

    // Shape palette
    pub shape_palette_bg: Color32,
    pub shape_palette_selected_bg: Color32,
    pub shape_palette_selected_stroke: Color32,

    // Horizontal grid scrollbar
    pub hscroll_track: Color32,
    pub hscroll_thumb: Color32,
    pub hscroll_thumb_active: Color32,

    // Minimap
    pub minimap_bg: Color32,
    pub minimap_viewport_fill: Color32,
    pub minimap_viewport_stroke: Color32,

    // Glyph pixels
    pub pixel_filled: Color32,

    // Glyph edit hover preview
    pub glyph_edit_preview: Color32,

    // Pixel selection
    pub pixel_selection: Color32,

    // Ref colors HSV parameters
    pub ref_hsv_s: f32,
    pub ref_hsv_v: f32,
}

impl Palette {
    pub fn dark() -> Self {
        Self {
            grid_bg: Color32::from_rgb(20, 22, 28),
            grid_off: Color32::from_rgb(42, 44, 52),
            grid_on: Color32::from_rgb(200, 205, 218),
            grid_ext_off: Color32::from_rgb(30, 32, 38),
            grid_ext_grid: Color32::from_rgba_premultiplied(55, 57, 65, 35),

            line_num: Color32::from_rgb(120, 120, 135),

            text_default: Color32::from_rgb(200, 200, 200),
            text_comment: Color32::from_rgb(87, 130, 70),
            text_heading: Color32::from_rgb(120, 175, 100),
            text_meta: Color32::from_rgb(70, 130, 180),
            text_header: Color32::from_rgb(190, 180, 130),
            text_ref: Color32::from_rgb(145, 145, 165),
            text_directive: Color32::from_rgb(150, 150, 115),
            text_directive2: Color32::from_rgb(130, 155, 140),

            error: Color32::from_rgb(220, 60, 60),
            link: Color32::from_rgb(80, 150, 255),

            selection: Color32::from_rgba_unmultiplied(60, 100, 180, 100),
            cursor_border: Color32::from_rgb(80, 140, 220),
            grid_border: Color32::from_rgba_unmultiplied(100, 180, 255, 180),

            shape_palette_bg: Color32::from_rgb(35, 35, 45),
            shape_palette_selected_bg: Color32::from_rgb(60, 80, 120),
            shape_palette_selected_stroke: Color32::from_rgb(100, 160, 240),

            hscroll_track: Color32::from_rgba_unmultiplied(60, 63, 75, 140),
            hscroll_thumb: Color32::from_rgba_unmultiplied(130, 138, 160, 200),
            hscroll_thumb_active: Color32::from_rgb(150, 190, 240),

            minimap_bg: Color32::from_rgb(25, 27, 35),
            minimap_viewport_fill: Color32::from_rgba_unmultiplied(100, 150, 220, 25),
            minimap_viewport_stroke: Color32::from_rgba_unmultiplied(120, 170, 240, 60),

            pixel_filled: Color32::from_rgb(210, 215, 230),

            glyph_edit_preview: Color32::from_rgba_unmultiplied(100, 180, 255, 120),

            pixel_selection: Color32::from_rgb(64, 140, 255),

            ref_hsv_s: 0.55,
            ref_hsv_v: 0.9,
        }
    }

    pub fn light() -> Self {
        let grid = Self::dark();
        Self {
            grid_bg: grid.grid_bg,
            grid_off: grid.grid_off,
            grid_on: grid.grid_on,
            grid_ext_off: grid.grid_ext_off,
            grid_ext_grid: grid.grid_ext_grid,

            line_num: Color32::from_rgb(200, 200, 210),

            text_default: Color32::from_rgb(30, 30, 30),
            text_comment: Color32::from_rgb(55, 110, 35),
            text_heading: Color32::from_rgb(40, 95, 25),
            text_meta: Color32::from_rgb(25, 95, 160),
            text_header: Color32::from_rgb(140, 115, 20),
            text_ref: Color32::from_rgb(95, 95, 130),
            text_directive: Color32::from_rgb(110, 105, 50),
            text_directive2: Color32::from_rgb(75, 115, 90),

            error: Color32::from_rgb(200, 35, 35),
            link: Color32::from_rgb(25, 90, 210),

            selection: Color32::from_rgba_unmultiplied(100, 150, 220, 80),
            cursor_border: grid.cursor_border,
            grid_border: grid.grid_border,

            shape_palette_bg: grid.shape_palette_bg,
            shape_palette_selected_bg: grid.shape_palette_selected_bg,
            shape_palette_selected_stroke: grid.shape_palette_selected_stroke,

            hscroll_track: Color32::from_rgba_unmultiplied(150, 155, 170, 120),
            hscroll_thumb: Color32::from_rgba_unmultiplied(90, 95, 115, 200),
            hscroll_thumb_active: Color32::from_rgb(40, 100, 190),

            minimap_bg: grid.minimap_bg,
            minimap_viewport_fill: grid.minimap_viewport_fill,
            minimap_viewport_stroke: grid.minimap_viewport_stroke,

            pixel_filled: grid.pixel_filled,

            glyph_edit_preview: grid.glyph_edit_preview,

            pixel_selection: grid.pixel_selection,

            ref_hsv_s: grid.ref_hsv_s,
            ref_hsv_v: grid.ref_hsv_v,
        }
    }

    /// The syntax colors, in a fixed order, so one palette's can be matched
    /// against another's. Only the theme-dependent text slots: everything else
    /// (the grid, the minimap's own chrome) is already one set of colors for
    /// both themes.
    fn syntax_slots(&self) -> [Color32; 8] {
        [
            self.text_default,
            self.text_comment,
            self.text_heading,
            self.text_meta,
            self.text_header,
            self.text_ref,
            self.text_directive,
            self.text_directive2,
        ]
    }

    /// What the dark palette calls the color this palette assigned — the color
    /// itself when the theme is already dark, and `text_default` for anything
    /// that is not a syntax color at all.
    ///
    /// For the minimap, which is drawn on a dark ground in both themes (see
    /// [`crate::editor::minimap`]) while the colors it repeats were resolved
    /// against the reader's theme: in light mode those are dark inks, and dark
    /// ink on a dark strip is nothing at all.
    pub fn dark_equivalent(&self, color: Color32) -> Color32 {
        let dark = Self::dark();
        match self.syntax_slots().iter().position(|c| *c == color) {
            Some(i) => dark.syntax_slots()[i],
            None => dark.text_default,
        }
    }

    pub fn for_theme(dark_mode: bool) -> Self {
        if dark_mode {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// The palette is cached under a context-global id on purpose: it is
    /// derived from the context's own theme, so every editor in it wants the
    /// same value. See [`crate::editor::ids`] for what *is* per-editor.
    pub fn store(ctx: &egui::Context) {
        let id = egui::Id::new("uniform_palette");
        let dark_id = egui::Id::new("uniform_palette_dark");
        let dark = ctx.theme() == egui::Theme::Dark;
        let stored_dark: Option<bool> = ctx.data(|d| d.get_temp(dark_id));
        if stored_dark == Some(dark) {
            return;
        }
        ctx.data_mut(|d| {
            d.insert_temp(id, Self::for_theme(dark));
            d.insert_temp(dark_id, dark);
        });
    }

    pub fn get(ui: &egui::Ui) -> Self {
        let id = egui::Id::new("uniform_palette");
        ui.ctx()
            .data(|d| d.get_temp::<Self>(id))
            .unwrap_or_else(Self::dark)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every syntax color the light theme assigns has a dark counterpart, and
    /// the dark theme's own colors map to themselves — the minimap is drawn
    /// with these whichever theme the reader is in.
    #[test]
    fn a_light_syntax_color_maps_to_its_dark_counterpart() {
        let light = Palette::light();
        let dark = Palette::dark();
        for (from, to) in light.syntax_slots().iter().zip(dark.syntax_slots()) {
            assert_eq!(light.dark_equivalent(*from), to);
        }
        for c in dark.syntax_slots() {
            assert_eq!(dark.dark_equivalent(c), c);
        }
        assert_eq!(
            light.dark_equivalent(Color32::from_rgb(1, 2, 3)),
            dark.text_default,
            "a color that is not a syntax slot falls back to the body color"
        );
    }
}
