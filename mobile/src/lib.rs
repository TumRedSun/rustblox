//! RustBlox mobile harness.
//!
//! The whole client lives in the `rustblox` library crate; this thin wrapper
//! provides the platform entry points the mobile runtimes expect:
//!
//! - **Android** — `android_main`, exported from `librustblox_mobile.so`
//!   (native-activity). The winit event loop is bound to the activity's
//!   `AndroidApp` handle before it is built; the window itself is created in
//!   `App::resumed`, dropped in `App::suspended`.
//! - **iOS** — the bin target (`.ipa` app executable) and the
//!   `rustblox_main` C entry for staticlib hosts; winit's iOS backend calls
//!   `UIApplicationMain` itself when the event loop runs.

/// JavaVM/activity handle for the JNI-based clipboard (Android).
#[cfg(target_os = "android")]
pub(crate) static APP: std::sync::OnceLock<android_activity::AndroidApp> =
    std::sync::OnceLock::new();

#[cfg(target_os = "android")]
mod android_clip;

#[cfg(target_os = "android")]
mod android_ui;

#[cfg(target_os = "ios")]
mod ios_clip;

/// Entry point of the iOS app executable (`mobile` bin target). Also used by
/// staticlib hosts through `rustblox_main`.
pub fn run_ios() {
    #[cfg(target_os = "ios")]
    rustblox::platform::register_platform_hooks(rustblox::platform::PlatformHooks {
        clipboard_get: Some(ios_clip::get),
        clipboard_set: Some(ios_clip::set),
        // winit's iOS backend implements UIKeyInput and delivers IME events
        // natively — no extra sync needed.
        ime_push: None,
        ime_pull: None,
        // Fullscreen is handled declaratively via Info.plist
        // (UIStatusBarHidden + UIViewControllerBasedStatusBarAppearance).
        window_ready: None,
        // iOS sandboxes the app (HOME points inside the bundle), so the
        // dirs-based config dir fallback works — no hook needed.
        app_data_dir: None,
    });
    rustblox::run_desktop();
}

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let _ = APP.set(app.clone());
    // Mirror the persistent log into the external files dir before anything
    // else runs, so a crash log can be pulled off the device without root
    // (see docs/MOBILE.md → Logging & crash reports).
    android_ui::register_log_mirror();
    rustblox::platform::register_platform_hooks(rustblox::platform::PlatformHooks {
        clipboard_get: Some(android_clip::get),
        clipboard_set: Some(android_clip::set),
        // TODO(roadmap): wire IME text sync when the app moves to the
        // GameActivity backend — winit 0.30 currently drops android-activity
        // TextEvents (commitText), so only hardware-style key events reach
        // the client today (see docs/MOBILE.md).
        ime_push: None,
        ime_pull: None,
        // Fired from App::resumed on every window (re)creation: hides the
        // status/navigation bars (sticky immersive) and enables drawing
        // under the camera cutout.
        window_ready: Some(android_ui::enter_immersive_mode),
        // App-private storage dir — sessions/settings survive restarts.
        app_data_dir: Some(android_ui::app_data_dir),
        // Settings → Logs → Save log / startup crash recovery: drops
        // rustblox-*.log into the public Downloads collection via
        // MediaStore, visible to any file manager without root.
        log_export: Some(android_ui::export_to_downloads),
    });
    // Crash-proofing before the event loop is built (its `.expect` can
    // panic on exotic devices): panic hook + persistent log file. run_with
    // re-calls both; they are idempotent.
    rustblox::logs::install_panic_hook();
    rustblox::logs::init_file();
    // Best-effort early apply (the window exists before the surface does);
    // the resumed hook re-applies it on every lifecycle change.
    android_ui::enter_immersive_mode();

    let session = rustblox::config::load_session();
    let event_loop = winit::event_loop::EventLoop::<rustblox::net::UserEvent>::with_user_event()
        .with_android_app(app)
        .build()
        .expect("failed to build the winit event loop");
    rustblox::run_with(event_loop, session);
}

/// Exported symbol for staticlib hosts (an Xcode project can call this).
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn rustblox_main() {
    run_ios();
}
