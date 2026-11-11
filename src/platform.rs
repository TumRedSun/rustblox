//! Platform hooks for mobile integration.
//!
//! The core client (`rustblox` lib) is platform-neutral; the mobile harness
//! crate (`rustblox-mobile`) registers function pointers here at startup so
//! desktop features (paste) and the soft-keyboard IME work on Android/iOS.
//! On desktop no hooks are registered and every call is a no-op.

use std::sync::{OnceLock, RwLock, RwLockReadGuard};

/// Function pointers provided by the platform harness.
#[derive(Clone, Copy, Default)]
pub struct PlatformHooks {
    /// Returns the current clipboard text, if any (Android JNI / iOS UIPasteboard).
    pub clipboard_get: Option<fn() -> Option<String>>,
    /// Puts `text` onto the system clipboard.
    pub clipboard_set: Option<fn(&str)>,
    /// Reflects the focused field's text into the platform IME (Android
    /// InputConnection), so the soft keyboard shows the right content.
    pub ime_push: Option<fn(&str)>,
    /// Reads back the IME text state (committed/composing text) — `Some`
    /// text means the platform has text to sync into the focused field.
    pub ime_pull: Option<fn() -> Option<String>>,
    /// Called whenever the native window was (re)created and is about to be
    /// shown (`App::resumed`). Mobile harnesses use it to (re)apply
    /// fullscreen/immersive window state — hide the status/navigation bars
    /// and let the surface draw under the display cutout. None = no-op.
    pub window_ready: Option<fn()>,
    /// Returns the app-private data directory (Android:
    /// NativeActivity's `internalDataPath`, e.g.
    /// `/data/data/<pkg>/files`). Used for persisting the session,
    /// accounts and settings — `dirs::config_dir()` is unusable on Android
    /// (no `$HOME`). None on desktop → the caller falls back to `dirs`.
    pub app_data_dir: Option<fn() -> Option<std::path::PathBuf>>,
    /// Saves `data` as `name` into the platform's user-visible Downloads
    /// location (Android: MediaStore.Downloads via JNI — reachable from any
    /// file manager without root). Not registered on desktop → the caller
    /// falls back to writing `dirs::download_dir()` directly; treat the
    /// result as best-effort either way.
    pub log_export: Option<fn(&str, &[u8]) -> bool>,
}

static HOOKS: OnceLock<RwLock<PlatformHooks>> = OnceLock::new();

/// Registers the platform hooks (called once from `android_main` / iOS main).
pub fn register_platform_hooks(hooks: PlatformHooks) {
    let lock = HOOKS.get_or_init(|| RwLock::new(PlatformHooks::default()));
    *lock.write().expect("platform hooks poisoned") = hooks;
}

fn hooks() -> RwLockReadGuard<'static, PlatformHooks> {
    HOOKS
        .get_or_init(|| RwLock::new(PlatformHooks::default()))
        .read()
        .expect("platform hooks poisoned")
}

/// Clipboard read through the platform backend (None on desktop — the app
/// falls back to arboard there).
pub fn clipboard_get() -> Option<String> {
    let f = hooks().clipboard_get?;
    f()
}

/// Clipboard write through the platform backend (no-op on desktop).
pub fn clipboard_set(text: &str) {
    if let Some(f) = hooks().clipboard_set {
        f(text);
    }
}

/// Pushes the focused field's text into the platform IME (no-op on desktop).
pub fn ime_push(text: &str) {
    if let Some(f) = hooks().ime_push {
        f(text);
    }
}

/// Pulls the current IME text state (no-op on desktop → None).
pub fn ime_pull() -> Option<String> {
    let f = hooks().ime_pull?;
    f()
}

/// The native window was (re)created (no-op on desktop).
pub fn window_ready() {
    if let Some(f) = hooks().window_ready {
        f();
    }
}

/// The platform's private data directory (None on desktop — callers fall
/// back to `dirs::config_dir()`).
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    let f = hooks().app_data_dir?;
    f()
}

/// Saves `data` as `name` into the user-visible Downloads location. Mobile
/// platforms go through the registered harness hook (Android: MediaStore).
/// Desktop has no hook, so it falls back to writing the file directly:
/// `dirs::download_dir()` (XDG config on Linux, the known Downloads folder
/// on Windows/macOS), then `$HOME/Downloads`, then `$HOME` itself.
pub fn export_log(name: &str, data: &[u8]) -> bool {
    if let Some(f) = hooks().log_export {
        return f(name, data);
    }
    let dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .or_else(dirs::home_dir);
    let Some(dir) = dir else { return false };
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    std::fs::write(dir.join(name), data).is_ok()
}
