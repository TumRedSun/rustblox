//! Runtime DataModel: the instance tree that the replication stream fills
//! and the 3D scene builder reads.
//!
//! Mirrors the Roblox object model in miniature: instances have a class,
//! a name, properties and parent/children links. Only the classes and
//! properties the renderer cares about are stored; everything else the
//! replication parser skips.

mod model;
mod scene_view;

pub use model::{DataModel, InstId, Instance, Prop, RefId};
pub use scene_view::{brick_color, build_ground, build_r6_rig, material_props, quat_rot};

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
