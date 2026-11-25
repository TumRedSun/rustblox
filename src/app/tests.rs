//! Tests for the keyboard modifier policy.

use super::char_input_allowed;
use winit::keyboard::ModifiersState;

#[test]
fn plain_and_shifted_typing_allowed() {
	assert!(char_input_allowed(ModifiersState::empty()));
	assert!(char_input_allowed(ModifiersState::SHIFT));
}

#[test]
fn shortcut_modifiers_rejected() {
	assert!(!char_input_allowed(ModifiersState::CONTROL));
	assert!(!char_input_allowed(ModifiersState::ALT));
	assert!(!char_input_allowed(ModifiersState::SUPER));
	assert!(!char_input_allowed(ModifiersState::CONTROL | ModifiersState::SHIFT));
}

#[test]
fn altgr_counts_as_text() {
	// AltGr is reported as Ctrl+Alt on Windows/EU layouts.
	assert!(char_input_allowed(ModifiersState::CONTROL | ModifiersState::ALT));
}
