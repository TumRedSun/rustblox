//! iOS system clipboard via `UIPasteboard`.

use objc2_foundation::NSString;
use objc2_ui_kit::UIPasteboard;

/// Returns the current clipboard text, if any.
pub fn get() -> Option<String> {
    unsafe {
        let pb = UIPasteboard::generalPasteboard();
        let s = pb.string()?;
        Some(s.to_string())
    }
}

/// Puts `text` onto the clipboard.
pub fn set(text: &str) {
    unsafe {
        let pb = UIPasteboard::generalPasteboard();
        pb.setString(Some(&NSString::from_str(text)));
    }
}
