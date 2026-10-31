fn main() {
    // the icon, and the manifest with common controls 6 (text box hints, themes) and per-monitor sharpness
    embed_resource::compile("lisdir.rc", embed_resource::NONE).manifest_optional().unwrap();
}
