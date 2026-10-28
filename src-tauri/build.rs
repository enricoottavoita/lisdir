fn main() {
    // ponytail: the app must be started elevated (MFT needs a raw volume handle); a requireAdministrator manifest breaks `cargo test`. Move scanning to an elevated service later.
    tauri_build::build()
}
