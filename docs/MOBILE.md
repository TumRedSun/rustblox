# Mobile support (experimental)

The `mobile` branch carries the work-in-progress Android and iOS support for
RustBlox. It is built by the **Mobile builds** workflow
(`.github/workflows/mobile.yml`), which runs on every push to `mobile` and on
demand. Passing builds publish all artifacts to the rolling
[`mobile`](../../releases/tag/mobile) pre-release — the mobile counterpart of
the `nightly` release on `main`.

## What ships today

| Artifact | Platform | What it is |
|----------|----------|------------|
| `rustblox-android-arm64.apk` | Android ARM64 | **Installable APK** (NativeActivity, debug-signed) |
| `rustblox-ios-arm64.ipa` | iOS ARM64 | App bundle in `Payload/` — **unsigned**, re-sign via AltStore/Sideloadly/Xcode to install |
| `rustblox-mobile-aarch64-linux-android.zip` | Android ARM64 | `librustblox_mobile.so` (for embedding into your own APK) |
| `rustblox-mobile-aarch64-apple-ios.zip` | iOS ARM64 | `librustblox_mobile.a` static library |

## Current feature status

| Part | Android | iOS |
|------|---------|-----|
| Compiles (release, ARM64) | ✅ `aarch64-linux-android` | ✅ `aarch64-apple-ios` |
| Installable package | ✅ APK (aapt2 + zipalign + apksigner) | ✅ .ipa (unsigned) |
| Touch input | ✅ tap = click, drag = list scroll | ✅ same code path |
| Clipboard | ✅ JNI → `ClipboardManager` | ✅ `UIPasteboard` |
| Soft keyboard show/hide | ✅ on field tap / tap-away | ✅ (winit UIKeyInput) |
| Text input | 🚧 hardware-style keys only (backspace, space, digits; BT keyboards fully work) | ✅ native IME events |
| Save/restore session | ✅ `.ROBLOSECURITY` config file | ✅ |

## How it works

The client itself lives in the `rustblox` library crate; the `mobile/`
workspace member is a thin harness crate (`rustblox-mobile`) that provides
the platform entry points and keeps Android/iOS dependencies out of the
desktop build:

- **desktop** — `rustblox::run_desktop()`, used by the root `src/main.rs`;
- **Android** — `android_main` binds winit to the activity's `AndroidApp`
  handle (`EventLoopBuilderExtAndroid::with_android_app`), then calls
  `rustblox::run_with`. The window/surface is created in `App::resumed` and
  dropped in `App::suspended` (required by the Android lifecycle);
- **iOS** — `rustblox_mobile::run_ios()` (bin target and `rustblox_main` C
  entry). winit's iOS backend calls `UIApplicationMain` itself when the event
  loop runs, so no Objective-C bootstrap is needed.

Shared plumbing lives in `rustblox::platform`: the harness registers function
pointers (clipboard get/set) that the client calls instead of arboard.

Touch input is handled in `App::handle_touch`: the first finger drives a
synthetic pointer through the same hit-testing pipeline as the mouse
(`pointer_pressed` / `pointer_moved` / `pointer_released`), and dragging
empty content scrolls the active list naturally (1:1 with the finger).

### Android APK (no Gradle)

The APK is assembled manually in CI — fully deterministic, no SDK license
prompts:

1. `cargo ndk` builds `librustblox_mobile.so` (NDK r27c, downloaded directly
   from dl.google.com);
2. `aapt2` (from the official build-tools zip) compiles `res/` and links
   `AndroidManifest.xml` + `android.jar` (platform-34) into an unsigned APK;
3. the `.so` is added under `lib/arm64-v8a/` with plain `zip`;
4. `zipalign` + `apksigner` sign it with the committed
   `mobile/android/debug.keystore` (standard debug credentials — dev builds
   only, `hasCode=false` means no Java code at all).

### iOS .ipa

The bin target (`rustblox-mobile`) is built for `aarch64-apple-ios` and
wrapped into `Payload/RustBlox.app` (Info.plist, PkgInfo, icons). The result
is an unsigned .ipa — Apple requires signing for device installation, so
re-sign it with AltStore, Sideloadly or a personal provisioning profile.

## Known limitations

- **Android soft keyboard text**: winit 0.30's Android backend drops
  android-activity `TextEvent`s (IME `commitText`), so typed letters do not
  reach the client yet. Hardware-style keys work (backspace, space, digits —
  many keyboards send these as key events), and Bluetooth keyboards work
  fully. Fixing this properly means switching the harness to the
  GameActivity backend (requires shipping Java classes in the APK) or
  waiting for winit to forward text events upstream.
- **iOS .ipa is unsigned** by design (no certificates in CI).
- **Session storage** uses the shared config directory; on Android it lives
  in the app sandbox, on iOS in the bundle's Documents dir.

## Building locally

```bash
# Android (requires NDK r26+ and cargo-ndk: cargo install cargo-ndk)
cargo ndk -t arm64-v8a build --release -p rustblox-mobile
# artifacts: target/aarch64-linux-android/release/librustblox_mobile.so

# iOS (requires macOS with Xcode)
rustup target add aarch64-apple-ios
cargo build --release --target aarch64-apple-ios -p rustblox-mobile
# artifacts: target/aarch64-apple-ios/release/rustblox-mobile (app exe),
#            target/aarch64-apple-ios/release/librustblox_mobile.a
```
