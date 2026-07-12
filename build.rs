fn main() {
    #[cfg(feature = "editor")]
    embed_resource::compile("app.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
