//! Retained widget state (text fields) + immediate-mode hit testing and
//! button drawing helpers shared by all screens.
//!
//! Layout: input model in `hit.rs`, the retained [`TextField`] state/editing
//! in `field.rs`, and everything that touches the framebuffer (field drawing,
//! buttons, spinner, status dot) in `widgets.rs`.

mod field;
mod hit;
mod widgets;

pub use field::TextField;
pub use hit::{FieldId, Hit, HitMap, Rect};
pub use widgets::{draw_button, draw_dot, draw_spinner, BtnStyle};
