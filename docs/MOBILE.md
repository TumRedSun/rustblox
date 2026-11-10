# Mobile support (experimental)

The `mobile` branch carries the work-in-progress Android and iOS support for
RustBlox. It is built by the **Mobile builds** workflow
(`.github/workflows/mobile.yml`), which runs on every push to `mobile` and on
demand. Passing builds publish both artifacts to the rolling
[`mobile`](../../releases/tag/mobile) pre-release — the mobile counterpart of
the `nightly` release on `main`.

## Current status

| Part | Android | iOS |
|------|---------|-----|
| Compiles (release, ARM64) | ✅ `aarch64-linux-android` | ✅ `aarch64-apple-ios` |
| Entry point wired | ✅ `android_main` (native-activity) | ✅ standard bin entry |
| Window / GPU surface lifecycle | ✅ created in `resumed`, dropped in `suspended` | ✅ same code path |
| Networking (ureq + rustls/ring) | ✅ | ✅ |
| Clipboard (arboard) | ❌ disabled (no mobile backend) | ❌ disabled |
| Touch input | 🚧 not mapped yet | 🚧 not mapped yet |
| Soft keyboard (IME) | ❌ | ❌ |
| Installable package (APK / .ipa) | ❌ | ❌ |

## What ships today

- `rustblox-mobile-aarch64-linux-android.zip` — `librustblox_mobile.so`
  (arm64-v8a), the cdylib the Android runtime loads; `android_main` is
  exported from the `rustblox-mobile` harness crate and bound to the winit
  event loop via `with_android_app`.
- `rustblox-mobile-aarch64-apple-ios.zip` — `librustblox_mobile.a`
  (aarch64), the static library to link into an Xcode project (entry point:
  `rustblox_main`).

These are **library artifacts**, not installable apps yet.

## How the port works

The client itself lives in the `rustblox` library crate; the `mobile/`
workspace member is a thin harness crate (`rustblox-mobile`) that only
provides the platform entry points, keeping Android/iOS dependencies out of
the desktop build:

- desktop — `rustblox::run_desktop()`, used by the root `src/main.rs`;
- Android — `rustblox_mobile`'s `android_main` binds winit to the activity's
  `AndroidApp` handle (`EventLoopBuilderExtAndroid::with_android_app`) before
  building the event loop, then calls `rustblox::run_with`;
- iOS — `rustblox_mobile`'s `rustblox_main` (C ABI) calls
  `rustblox::run_desktop`; winit's iOS backend needs no extra handle.

Both paths converge in `rustblox::run_with`, which spawns the network worker
and runs `app::App`. The Android lifecycle is already respected:

- `App::resumed` creates the window and the pixel surface (on Android this
  fires *after* the activity is up, which is exactly when window creation is
  allowed);
- `App::suspended` drops the surface and window, because the GPU surfaces die
  with the native window on suspend; the next `resumed` recreates everything.

`arboard` (desktop clipboard) is a target-specific dependency and the paste
path is compiled out on mobile targets.

## Roadmap

1. **Touch input** — map `winit` `WindowEvent::Touch` onto the existing
   pointer/hit-testing pipeline (tap = click, drag = scroll).
2. **Text input** — soft keyboard via platform IME events for the login and
   search fields (paste stays a no-op until then).
3. **Android packaging** — Gradle template (GameActivity or NativeActivity)
   that loads `librustblox_mobile.so` and produces an APK; `cargo ndk`
   already covers the Rust side.
4. **iOS packaging** — minimal Xcode project linking `librustblox_mobile.a`
   with a Metal view; device deployment needs signing.
5. **UI adaptation** — portrait layouts and density-aware scaling (the current
   UI targets desktop window sizes ≥ 880×600 logical pixels).

## Building locally

```bash
# Android (requires NDK r26+ and cargo-ndk: cargo install cargo-ndk)
cargo ndk -t arm64-v8a build --release -p rustblox-mobile
# artifact: target/aarch64-linux-android/release/librustblox_mobile.so

# iOS (requires macOS with Xcode)
rustup target add aarch64-apple-ios
cargo build --release --target aarch64-apple-ios -p rustblox-mobile
# artifact: target/aarch64-apple-ios/release/librustblox_mobile.a
```
