//! RustBlox mobile harness.
//!
//! The whole client lives in the `rustblox` library crate; this thin wrapper
//! provides the platform entry points the mobile runtimes expect:
//!
//! - **Android** — `android_main`, exported from `librustblox_mobile.so`
//!   (native-activity). The winit event loop is bound to the activity's
//!   `AndroidApp` handle before it is built; the window itself is created in
//!   `App::resumed`, dropped in `App::suspended`.
//! - **iOS** — `rustblox_main`, exported from `librustblox_mobile.a` and
//!   callable from a Swift/ObjC host application.

#[cfg(target_os = "android")]
#[no_mangle]
fn android_main(app: android_activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    let session = rustblox::config::load_session();
    let event_loop = winit::event_loop::EventLoop::<rustblox::net::UserEvent>::with_user_event()
        .with_android_app(app)
        .build()
        .expect("failed to build the winit event loop");
    rustblox::run_with(event_loop, session);
}

/// C entry point for an iOS host application (call this from Swift/ObjC
/// after the UI is ready). winit's iOS backend uses the standard event loop.
#[cfg(target_os = "ios")]
#[no_mangle]
pub extern "C" fn rustblox_main() {
    rustblox::run_desktop();
}
