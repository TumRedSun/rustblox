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
