//! Puts the app icon inside the Windows exe. The window sets its own icon at runtime, but
//! Explorer, the installer's shortcuts and taskbar pins read it from the file.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    // only a Windows exe carries resources (the Android library is built from a Windows PC too)
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed_resource::compile("assets/icon.rc", embed_resource::NONE).manifest_optional().unwrap();
}
