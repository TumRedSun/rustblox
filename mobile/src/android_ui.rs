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

/// App-private data directory (NativeActivity `internalDataPath`, typically
/// `/data/data/<package>/files`). Registered as the `app_data_dir` platform
/// hook so sessions, saved accounts and settings persist across restarts —
/// `dirs::config_dir()` is useless here because Android sets no `$HOME`.
pub fn app_data_dir() -> Option<std::path::PathBuf> {
    crate::APP.get().and_then(|app| app.internal_data_path())
}

/// Register the log mirror in the app's *external* files dir
/// (`/storage/emulated/0/Android/data/<package>/files/rustblox/logs/`).
///
/// The primary log lives in app-private storage (see [`app_data_dir`]),
/// which stock Android does not let the user open without root or adb.
/// The external dir is writable by the app unconditionally and reachable
/// from a file manager (Android ≤ 10) or USB from a PC — so a crash log
/// can leave the device even without root. Best effort: if the shared
/// storage is unmounted or restricted, the mirror silently stays absent
/// and the in-app Settings → Logs + internal file still work.
pub fn register_log_mirror() {
    let Some(path) = crate::APP.get().and_then(|app| app.external_data_path()) else {
        return;
    };
    rustblox::logs::add_mirror_file(&path.join("rustblox").join("logs").join("client.log"));
}

/// Saves `data` as `name` into the public Downloads collection via
/// MediaStore (API 29+, no permissions needed for the app's own files).
///
/// This is the user-facing export path: files land in `Download/` where
/// ANY file manager (and any chat app's attachment picker) can see them,
/// unlike the app-private or external-files locations. On API 26–28,
/// where MediaStore.Downloads does not exist, falls back to a direct
/// write into `Environment.getExternalStoragePublicDirectory(Download)`
/// (needs WRITE_EXTERNAL_STORAGE granted; otherwise fails silently).
///
/// Runs on the app thread (Settings button / startup recovery), never
/// inside the panic hook — JNI from a panicking thread is too fragile.
pub fn export_to_downloads(name: &str, data: &[u8]) -> bool {
    let Some(app) = crate::APP.get() else { return false };
    unsafe {
        let Ok(vm) = jni::JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) else {
            return false;
        };
        let Ok(mut guard) = vm.attach_current_thread() else { return false };
        let env: &mut jni::JNIEnv = &mut guard;
        let activity =
            jni::objects::JObject::from_raw(app.activity_as_ptr() as *mut jni::sys::_jobject);

        if export_via_mediastore(env, &activity, name, data) {
            return true;
        }
        let _ = clear(env);
        export_via_legacy_download_dir(env, &activity, name, data)
    }
}

/// API 29+: insert into MediaStore.Downloads, then write through the
/// ContentResolver's OutputStream.
fn export_via_mediastore(
    env: &mut jni::JNIEnv,
    activity: &jni::objects::JObject<'static>,
    name: &str,
    data: &[u8],
) -> bool {
    use jni::objects::JValue;

    // MediaStore.Downloads.EXTERNAL_CONTENT_URI (static field, API 29+).
    let Ok(uri) = env
        .get_static_field(
            "android/provider/MediaStore$Downloads",
            "EXTERNAL_CONTENT_URI",
            "Landroid/net/Uri;",
        )
        .and_then(|v| v.l())
    else {
        return false;
    };
    let Ok(values) = env.new_object("android/content/ContentValues", "()V", &[]) else {
        return false;
    };
    let Ok(jname) = env.new_string(name) else { return false };
    let Ok(mime) = env.new_string("text/plain") else { return false };
    for (k, v) in [("_display_name", &jname), ("mime_type", &mime)] {
        let Ok(jkey) = env.new_string(k) else { return false };
        if env
            .call_method(
                &values,
                "put",
                "(Ljava/lang/String;Ljava/lang/String;)V",
                &[JValue::Object(&jkey), JValue::Object(v)],
            )
            .is_err()
        {
            return false;
        }
    }
    let Ok(resolver) = env
        .call_method(activity, "getContentResolver", "()Landroid/content/ContentResolver;", &[])
        .and_then(|v| v.l())
    else {
        return false;
    };
    let Ok(new_uri) = env
        .call_method(
            &resolver,
            "insert",
            "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
            &[JValue::Object(&uri), JValue::Object(&values)],
        )
        .and_then(|v| v.l())
    else {
        return false;
    };
    if new_uri.is_null() {
        return false;
    }
    let Ok(out) = env
        .call_method(
            &resolver,
            "openOutputStream",
            "(Landroid/net/Uri;)Ljava/io/OutputStream;",
            &[JValue::Object(&new_uri)],
        )
        .and_then(|v| v.l())
    else {
        return false;
    };
    if out.is_null() {
        return false;
    }
    let Ok(arr) = env.byte_array_from_slice(data) else { return false };
    let Ok(()) = env.call_method(
        &out,
        "write",
        "([B)V",
        &[JValue::Object(&arr)],
    ) else {
        return false;
    };
    // close() must run even if the write path already failed above. Local
    // JNI references are freed when the attach guard pops at return.
    let _ = env.call_method(&out, "close", "()V", &[]);
    true
}

/// API 26–28 fallback: direct write into the public Download directory.
/// Best effort — needs WRITE_EXTERNAL_STORAGE on those versions; silently
/// returns false when the sandbox denies it.
fn export_via_legacy_download_dir(
    env: &mut jni::JNIEnv,
    _activity: &jni::objects::JObject<'static>,
    name: &str,
    data: &[u8],
) -> bool {
    use jni::objects::JValue;

    let Ok(dir_name) = env
        .get_static_field("android/os/Environment", "DIRECTORY_DOWNLOADS", "Ljava/lang/String;")
        .and_then(|v| v.l())
    else {
        return false;
    };
    let Ok(dir) = env
        .call_static_method(
            "android/os/Environment",
            "getExternalStoragePublicDirectory",
            "(Ljava/lang/String;)Ljava/io/File;",
            &[JValue::Object(&dir_name)],
        )
        .and_then(|v| v.l())
    else {
        return false;
    };
    if dir.is_null() {
        return false;
    }
    let Ok(jname) = env.new_string(name) else { return false };
    // new File(dir, name)
    let Ok(file) = env.new_object(
        "java/io/File",
        "(Ljava/io/File;Ljava/lang/String;)V",
        &[JValue::Object(&dir), JValue::Object(&jname)],
    ) else {
        return false;
    };
    let Ok(fos) = env
        .new_object("java/io/FileOutputStream", "(Ljava/io/File;)V", &[JValue::Object(&file)])
    else {
        return false;
    };
    let Ok(arr) = env.byte_array_from_slice(data) else { return false };
    if env
        .call_method(&fos, "write", "([B)V", &[JValue::Object(&arr)])
        .is_err()
    {
        return false;
    }
    let _ = env.call_method(&fos, "close", "()V", &[]);
    true
}
