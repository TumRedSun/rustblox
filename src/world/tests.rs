//! DataModel tree, property and scene-building tests.

use super::*;
use crate::gfx::scene::SceneView;

#[test]
fn create_remove_tree() {
	let mut dm = DataModel::new();
	let r = dm.create(100, "DataModel", "", -1);
	let w = dm.create(101, "Workspace", "", 100);
	let p = dm.create(102, "Part", "Baseplate", 101);
	assert_eq!(dm.len(), 3);
	assert_eq!(dm.get(p).unwrap().name, "Baseplate");
	assert_eq!(dm.get(w).unwrap().children, vec![p]);
	assert!(dm.remove(101));
	assert_eq!(dm.len(), 1);
	assert!(dm.get(p).is_none());
	assert!(dm.get(r).is_some());
}

#[test]
fn props_and_scene() {
	let mut dm = DataModel::new();
	dm.create(-10, "DataModel", "", -1);
	let w = dm.create(-11, "Workspace", "", -10);
	let _p = dm.create(-12, "Part", "Brick", -11);
	dm.set_prop(
		-12,
		"CFrame",
		Prop::CFrame { pos: [0.0, 5.0, 0.0], rot: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] },
	);
	dm.set_prop(-12, "Size", Prop::Vec3([4.0, 4.0, 4.0]));
	dm.set_prop(-12, "Color3uint8", Prop::Color3u8([255, 0, 0]));
	dm.set_prop(-12, "Material", Prop::Token(1280)); // Neon
	let mut scene = SceneView::default();
	let n = dm.build_scene(&mut scene);
	assert_eq!(n, 1);
	assert_eq!(scene.parts[0].pos, [0.0, 5.0, 0.0]);
	assert_eq!(scene.parts[0].color, [1.0, 0.0, 0.0, 1.0]);
	assert_eq!(scene.parts[0].emissive, 0.9);
	let _ = w;
}

#[test]
fn lighting_time_of_day() {
	let mut dm = DataModel::new();
	dm.create(-1, "Lighting", "", -1);
	dm.set_prop(-1, "ClockTime", Prop::F32(12.0));
	let mut scene = SceneView::default();
	dm.build_scene(&mut scene);
	// Noon: sun high.
	assert!(scene.env.sun_dir[1] > 0.9);
}

#[test]
fn brick_color_known() {
	let c = brick_color(26); // Really black
	assert_eq!(c, [0.0, 0.0, 0.0]);
	let c = brick_color(11); // Bright red
	assert!((c[0] - 0x75 as f32 / 255.0).abs() < 1e-5);
}

#[test]
fn r6_rig_six_parts() {
	let mut out = Vec::new();
	let body = crate::roblox::join::AvatarAppearance::default();
	let n = build_r6_rig([0.0, 0.0, 0.0], 0.0, &body, &mut out);
	assert_eq!(n, 6);
	assert_eq!(out.len(), 6);
}
