//! Android's own services the app reaches through the activity (Java, android/app/.../MainActivity).

use std::sync::OnceLock;

use jni::objects::{JObject, JString};
use jni::JavaVM;
use winit::platform::android::activity::AndroidApp;

static APP: OnceLock<AndroidApp> = OnceLock::new();

/// Keeps the activity's handle for later calls into Java, and sets up the keyboard: plain text,
/// no suggestions or autocorrect (API keys, logins), never the full-screen editing box.
pub fn set_app(app: AndroidApp) {
    use winit::platform::android::activity::input::{ImeOptions, InputType, TextInputAction};
    app.set_ime_editor_info(
        InputType::TYPE_CLASS_TEXT | InputType::TYPE_TEXT_FLAG_NO_SUGGESTIONS | InputType::TYPE_TEXT_VARIATION_VISIBLE_PASSWORD,
        TextInputAction::Done,
        ImeOptions::IMG_FLAG_NO_EXTRACT_UI | ImeOptions::IME_FLAG_NO_FULLSCREEN,
    );
    let _ = APP.set(app);
}

/// The keyboard's text buffer, and whether it is in the middle of composing a word.
pub fn keyboard_text() -> Option<(String, bool)> {
    let state = APP.get()?.text_input_state();
    let composing = state.compose_region.is_some_and(|r| r.start != r.end);
    Some((state.text, composing))
}

/// Replaces the keyboard's text buffer, the cursor at its end.
pub fn set_keyboard_text(text: &str) {
    use winit::platform::android::activity::input::{TextInputState, TextSpan};
    if let Some(app) = APP.get() {
        let end = text.len();
        app.set_text_input_state(TextInputState { text: text.to_string(), selection: TextSpan { start: end, end }, compose_region: None });
    }
}

/// The text on Android's clipboard (`MainActivity.clipboardText()`), or why there is none.
pub fn clipboard_text() -> Result<String, String> {
    let app = APP.get().ok_or("The app isn't ready yet")?;
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) }.map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    // the activity stays alive for the whole app: a borrowed reference, never deleted
    let activity = unsafe { JObject::from_raw(app.activity_as_ptr() as jni::sys::jobject) };
    let value = env.call_method(&activity, "clipboardText", "()Ljava/lang/String;", &[]);
    let text = match value.and_then(|v| v.l()) {
        Ok(t) if !t.is_null() => t,
        Ok(_) => return Err("Nothing copied".into()),
        Err(e) => {
            // a Java exception would otherwise stay pending on this thread
            let _ = env.exception_clear();
            tracing::warn!("clipboard: {e}");
            return Err(format!("Couldn't read the clipboard: {e}"));
        }
    };
    let text = JString::from(text);
    let s: String = env.get_string(&text).map_err(|e| e.to_string())?.into();
    if s.is_empty() {
        return Err("Nothing copied".into());
    }
    Ok(s)
}
