pub mod cluster;
pub mod harfbuzz;
pub mod rasterizer;
pub mod widget;

#[cfg(target_os = "macos")]
pub mod coretext;

#[cfg(target_os = "windows")]
pub mod directwrite;

#[derive(Clone, Debug)]
pub struct Feature {
    pub tag: [u8; 4],
    pub value: u32,
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct ShapedGlyph {
    pub glyph_id: u16,
    pub cluster: usize,
    pub x_advance: f32,
    #[allow(dead_code)]
    pub y_advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug)]
pub struct ShapeError(pub String);

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub trait ShaperBackend {
    fn name(&self) -> &'static str;
    fn shape(
        &self,
        font_data: &[u8],
        text: &str,
        upm: u16,
        features: &[Feature],
    ) -> Result<Vec<ShapedGlyph>, ShapeError>;
}

pub fn available_backends() -> Vec<Box<dyn ShaperBackend>> {
    let mut v: Vec<Box<dyn ShaperBackend>> = vec![Box::new(harfbuzz::HarfBuzzBackend)];
    #[cfg(target_os = "macos")]
    v.push(Box::new(coretext::CoreTextBackend));
    #[cfg(target_os = "windows")]
    v.push(Box::new(directwrite::DirectWriteBackend));
    v
}
