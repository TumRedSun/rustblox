//! Android system clipboard via JNI (`android.content.ClipboardManager`).
//!
//! The APK ships with `hasCode=false` (NativeActivity, no Java classes of
//! ours) — clipboard access goes through JNI using the JavaVM / activity
//! handles that android-activity exposes. Every call is best-effort: any
//! JNI error simply yields `None` / a no-op instead of failing the app.

use android_activity::AndroidApp;
use jni::objects::{JObject, JString, JValue};
use jni::{JNIEnv, JavaVM};

/// Returns the primary clipboard text, if any.
pub fn get() -> Option<String> {
    let app = crate::APP.get()?;
    unsafe {
        let vm = JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM).ok()?;
        let mut guard = vm.attach_current_thread().ok()?;
        let env: &mut JNIEnv = &mut guard;
        // Global ref owned by the activity itself — do not wrap or delete.
        let activity = JObject::from_raw(app.activity_as_ptr() as *mut jni::sys::_jobject);

        let service = env.new_string("clipboard").ok()?;
        let cb = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&service)],
            )
            .ok()?
            .l()
            .ok()?;
        if cb.is_null() {
            return None;
        }
        let clip = env
            .call_method(&cb, "getPrimaryClip", "()Landroid/content/ClipData;", &[])
            .ok()?
            .l()
            .ok()?;
        if clip.is_null() {
            return None;
        }
        let item = env
            .call_method(
                &clip,
                "getItemAt",
                "(I)Landroid/content/ClipData$Item;",
                &[JValue::Int(0)],
            )
            .ok()?
            .l()
            .ok()?;
        if item.is_null() {
            return None;
        }
        let seq = env
            .call_method(&item, "getText", "()Ljava/lang/CharSequence;", &[])
            .ok()?
            .l()
            .ok()?;
        if seq.is_null() {
            return None;
        }
        let s = env
            .call_method(&seq, "toString", "()Ljava/lang/String;", &[])
            .ok()?
            .l()
            .ok()?;
        let jstr = JString::from(s);
        Some(env.get_string(&jstr).ok()?.into())
    }
}

/// Puts `text` onto the clipboard.
pub fn set(text: &str) {
    let Some(app) = crate::APP.get() else { return };
    unsafe {
        let Ok(vm) = JavaVM::from_raw(app.vm_as_ptr() as *mut jni::sys::JavaVM) else {
            return;
        };
        let Ok(mut guard) = vm.attach_current_thread() else {
            return;
        };
        let env: &mut JNIEnv = &mut guard;
        let activity = JObject::from_raw(app.activity_as_ptr() as *mut jni::sys::_jobject);

        let Some(service) = env.new_string("clipboard") else { return };
        let Ok(cb) = env
            .call_method(
                &activity,
                "getSystemService",
                "(Ljava/lang/String;)Ljava/lang/Object;",
                &[JValue::Object(&service)],
            )
            .and_then(|v| v.l())
        else {
            return;
        };
        if cb.is_null() {
            return;
        }
        let Ok(label) = env.new_string("RustBlox") else { return };
        let Ok(jtext) = env.new_string(text) else { return };
        let Ok(clip) = env
            .call_static_method(
                "android/content/ClipData",
                "newPlainText",
                "(Ljava/lang/CharSequence;Ljava/lang/CharSequence;)Landroid/content/ClipData;",
                &[JValue::Object(&label), JValue::Object(&jtext)],
            )
            .and_then(|v| v.l())
        else {
            return;
        };
        let _ = env.call_method(
            &cb,
            "setPrimaryClip",
            "(Landroid/content/ClipData;)V",
            &[JValue::Object(&clip)],
        );
    }
}
