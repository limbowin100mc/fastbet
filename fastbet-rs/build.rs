//! Puts the app icon inside the Windows exe. The window sets its own icon at runtime, but
//! Explorer, the installer's shortcuts and taskbar pins read it from the file.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    embed_resource::compile("assets/icon.rc", embed_resource::NONE).manifest_optional().unwrap();
}
