//! Android fullscreen/immersive window state via JNI (best-effort).
//!
//! Runs on the UI thread (android_main and winit's `resumed` both run there)
//! and applies three things so the client is usable on phones:
//!
//!  * **Sticky immersive** — the status bar and the navigation bar
//!    (home / back / recents buttons) are hidden; a swipe reveals them
//!    transiently and they auto-hide again;
//!  * **Edge-to-edge layout** — the client surface spans the whole screen
//!    instead of being pushed below the system bars;
//!  * **Cutout shortEdges** (API 28+) — the window draws under the camera
//!    cutout: the notch simply overlays the corner of the app instead of
//!    shrinking/offsetting the client UI.
//!
//! Every step is optional: any JNI failure skips that step silently (the
//! pending Java exception is cleared so it can never poison later calls).

use android_activity::AndroidApp;
use jni::objects::{JObject, JValue};
use jni::{JNIEnv, JavaVM};

// android.view.View system-UI flags (legacy path, honored API 16–34).
const VIEW_HIDE_NAVIGATION: i32 = 0x0002;
const VIEW_FULLSCREEN: i32 = 0x0004;
const VIEW_LAYOUT_STABLE: i32 = 0x0100;
const VIEW_LAYOUT_HIDE_NAVIGATION: i32 = 0x0200;
const VIEW_LAYOUT_FULLSCREEN: i32 = 0x0400;
const VIEW_IMMERSIVE_STICKY: i32 = 0x1000;
const IMMERSIVE_FLAGS: i32 = VIEW_HIDE_NAVIGATION
    | VIEW_FULLSCREEN
    | VIEW_LAYOUT_STABLE
    | VIEW_LAYOUT_HIDE_NAVIGATION
    | VIEW_LAYOUT_FULLSCREEN
    | VIEW_IMMERSIVE_STICKY;

// WindowManager.LayoutParams.layoutInDisplayCutoutMode (API 28+).
const CUTOUT_MODE_SHORT_EDGES: i32 = 1;
// WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE (API 30+).
const BEHAVIOR_TRANSIENT_BY_SWIPE: i32 = 2;
// WindowInsets.Type.systemBars() fallback mask: statusBars|captionBar|navBars.
const TYPE_SYSTEM_BARS_FALLBACK: i32 = 0b1011;

/// Applies the fullscreen/immersive state. Registered as the
/// `window_ready` platform hook (fired from `App::resumed` on every window
/// (re)creation) and called once at startup.
pub fn enter_immersive_mode() {
    let Some(app) = crate::APP.get() else { return };
    unsafe {
        let Ok(vm) = JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) else {
            return;
        };
        let Ok(mut guard) = vm.attach_current_thread() else {
            return;
        };
        let env: &mut JNIEnv = &mut guard;
        // Global ref owned by the activity itself — do not wrap or delete.
        let activity = JObject::from_raw(app.activity_as_ptr() as *mut jni::sys::_jobject);
        apply(env, &activity);
    }
}

fn apply(env: &mut JNIEnv, activity: &JObject) {
    let Ok(window) = env
        .call_method(activity, "getWindow", "()Landroid/view/Window;", &[])
        .and_then(|v| v.l())
    else {
        clear(env);
        return;
    };
    clear(env);
    if window.is_null() {
        return;
    }

    let sdk = env
        .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")
        .and_then(|v| v.i())
        .unwrap_or(0);
    clear(env);

    // Camera cutout: let the client draw under it (API 28+).
    if sdk >= 28 {
        if let Ok(attrs) = env
            .call_method(
                &window,
                "getAttributes",
                "()Landroid/view/WindowManager$LayoutParams;",
                &[],
            )
            .and_then(|v| v.l())
        {
            let _ = env.set_field(
                &attrs,
                "layoutInDisplayCutoutMode",
                "I",
                JValue::Int(CUTOUT_MODE_SHORT_EDGES),
            );
            let _ = env.call_method(
                &window,
                "setAttributes",
                "(Landroid/view/WindowManager$LayoutParams;)V",
                &[JValue::Object(&attrs)],
            );
        }
        clear(env);
    }

    // Modern edge-to-edge + system-bar hiding (API 30+).
    if sdk >= 30 {
        let _ = env.call_method(
            &window,
            "setDecorFitsSystemWindows",
            "(Z)V",
            &[JValue::Bool(0)],
        );
        clear(env);
        if let Ok(ctrl) = env
            .call_method(
                &window,
                "getInsetsController",
                "()Landroid/view/WindowInsetsController;",
                &[],
            )
            .and_then(|v| v.l())
        {
            if !ctrl.is_null() {
                let mask = env
                    .call_static_method(
                        "android/view/WindowInsets$Type",
                        "systemBars",
                        "()I",
                        &[],
                    )
                    .and_then(|v| v.i())
                    .unwrap_or(TYPE_SYSTEM_BARS_FALLBACK);
                clear(env);
                let _ = env.call_method(
                    &ctrl,
                    "setSystemBarsBehavior",
                    "(I)V",
                    &[JValue::Int(BEHAVIOR_TRANSIENT_BY_SWIPE)],
                );
                let _ = env.call_method(&ctrl, "hide", "(I)V", &[JValue::Int(mask)]);
            }
        }
        clear(env);
    }

    // Legacy sticky-immersive flags — the fallback path for API 26–29 and a
    // belt-and-braces re-apply on newer versions (deprecated but honored).
    if let Ok(decor) = env
        .call_method(&window, "getDecorView", "()Landroid/view/View;", &[])
        .and_then(|v| v.l())
    {
        let _ = env.call_method(
            &decor,
            "setSystemUiVisibility",
            "(I)V",
            &[JValue::Int(IMMERSIVE_FLAGS)],
        );
    }
    clear(env);
}

/// Clears any pending Java exception so it can never poison later JNI calls
/// (the winit backend calls JNI on the same thread).
fn clear(env: &mut JNIEnv) {
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
    }
}
