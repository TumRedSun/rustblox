//! Scene building: instance properties → renderable boxes, palettes,
//! lighting and the fallback ground.

use super::model::{Instance, Prop};
use crate::gfx::math::{quat_to_mat3, Vec3};
use crate::gfx::scene::{Environment, ScenePart, SceneView};

/// BrickColor palette id → RGB (the 64 classic BrickColors, condensed to the
/// most common entries; unknown ids fall back to medium grey).
pub fn brick_color(id: u64) -> [f32; 3] {
	let (r, g, b) = match id {
		1 => (0x7f, 0x7f, 0x7f),  // White
		2 => (0xc4, 0xc4, 0xc4),  // Grey
		3 => (0x99, 0x99, 0x99),  // Light yellow
		5 => (0xa1, 0xc4, 0x8c),  // Brick yellow
		6 => (0x6b, 0x74, 0x7e),  // Light blue
		7 => (0x01, 0x9e, 0xd6),  // Bright blue
		8 => (0xf5, 0xcd, 0x30),  // Bright yellow
		9 => (0xd7, 0xc0, 0x9a),  // Earth orange
		10 => (0x6b, 0x32, 0x7c), // Bright violet
		11 => (0x75, 0x1f, 0x1f), // Bright red
		12 => (0x4b, 0x97, 0x4b), // Bright green
		13 => (0x5a, 0x5a, 0x5a), // Medium grey
		14 => (0x1b, 0x2a, 0x35), // Dark navy
		15 => (0xa3, 0xa2, 0xa5), // Light grey
		16 => (0x27, 0xa1, 0x62), // Cyan
		17 => (0xab, 0xd2, 0xc2), // Sand
		18 => (0x00, 0x8f, 0x9c), // Teal
		19 => (0xa1, 0x5f, 0x1d), // Dark orange
		20 => (0x6a, 0x4f, 0x30), // Brown
		21 => (0x50, 0x2a, 0x1d), // Dark brown
		22 => (0x1f, 0x3d, 0x2f), // Deep green
		23 => (0x11, 0x3c, 0x53), // Midnight blue
		24 => (0x7f, 0x10, 0x10), // Maroon
		25 => (0x3c, 0x3c, 0x3c), // Dark grey
		26 => (0x00, 0x00, 0x00), // Really black
		27 => (0xf2, 0xf3, 0xf3), // Institutional white
		28 => (0x20, 0x42, 0x7e), // Navy blue
		29 => (0x5b, 0x78, 0xc8), // Steel blue
		30 => (0x7c, 0x9d, 0xe8), // Cornflower blue
		31 => (0x8c, 0x88, 0xc8), // Royal purple
		32 => (0xa7, 0x8f, 0xd1), // Lilac
		33 => (0xf8, 0xba, 0xa0), // Salmon
		34 => (0xfa, 0x86, 0x7a), // Coral
		35 => (0xf6, 0x4b, 0x3e), // Orange red
		36 => (0xf5, 0xd4, 0x44), // Bright orange
		37 => (0xe2, 0x9c, 0x1e), // Gold
		38 => (0x9c, 0x8b, 0x2c), // Olive
		39 => (0xa4, 0xbd, 0x47), // Lime green
		40 => (0x74, 0xb6, 0x3c), // Bright green
		41 => (0x53, 0x9b, 0x34), // Sea green
		42 => (0x1c, 0x5b, 0x2f), // Forest green
		43 => (0x79, 0xc2, 0x7c), // Mint
		44 => (0x0b, 0xb8, 0xe2), // Bright cyan
		45 => (0x80, 0xbb, 0xdb), // Baby blue
		46 => (0x9f, 0xd5, 0xec), // Pastel blue
		47 => (0xc7, 0xd4, 0xe5), // Fog blue
		48 => (0xd4, 0xd7, 0xd9), // Silver
		49 => (0x6d, 0x6e, 0x71), // Slate
		50 => (0x21, 0x29, 0x30), // Charcoal
		_ => (0xa1, 0xc4, 0x8c),
	};
	[r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0]
}

/// Map a Roblox Material token to (roughness, emissive).
pub fn material_props(token: u32) -> (f32, f32) {
	// Token values follow the Roblox Material enum (subset).
	match token {
		256 => (0.05, 0.0),  // Plastic (smooth-ish)
		272 => (0.9, 0.0),   // Wood
		288 => (0.95, 0.0),  // Slate
		304 => (0.3, 0.0),   // Concrete
		336 => (0.1, 0.0),   // Metal
		512 => (0.35, 0.0),  // Granite
		784 => (0.7, 0.0),   // Brick
		800 => (0.9, 0.0),   // Pebble
		1088 => (0.85, 0.0), // Sand
		1136 => (0.6, 0.0),  // Fabric
		1280 => (0.15, 0.9), // Neon
		1344 => (0.5, 0.0),  // Glass
		1360 => (0.4, 0.0),  // ForceField
		_ => (0.75, 0.0),
	}
}

pub(super) fn part_from_instance(inst: &Instance) -> Option<ScenePart> {
	// Position: CFrame preferred, Position fallback (axis-aligned).
	let (pos, rot) = match inst.prop("CFrame") {
		Some(Prop::CFrame { pos, rot }) => (*pos, *rot),
		_ => {
			let p = inst.prop("Position").and_then(|p| p.as_v3()).unwrap_or([0.0; 3]);
			(p, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
		}
	};
	let size = inst.prop("Size").and_then(|p| p.as_v3()).unwrap_or([4.0, 1.2, 2.0]);

	let color: [f32; 3] =
		match (inst.prop("Color3uint8"), inst.prop("Color"), inst.prop("BrickColor")) {
			(Some(Prop::Color3u8(c)), _, _) => {
				[c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
			}
			(_, Some(Prop::Color3(c)), _) => *c,
			(_, _, Some(Prop::I32(id))) => brick_color(*id as u64),
			(_, _, Some(Prop::Token(id))) => brick_color(*id as u64),
			_ => [0.63, 0.77, 0.55],
		};
	let material = match inst.prop("Material") {
		Some(Prop::Token(t)) => *t,
		Some(Prop::I32(t)) => *t as u32,
		_ => 256,
	};
	let (roughness, emissive) = material_props(material);
	let transparency =
		inst.prop("Transparency").and_then(|p| p.as_f32()).unwrap_or(0.0).clamp(0.0, 1.0);
	if transparency >= 1.0 {
		return None; // fully invisible
	}

	Some(ScenePart {
		pos,
		size,
		rot,
		color: [color[0], color[1], color[2], 1.0 - transparency],
		roughness,
		emissive,
		cast_shadow: true,
	})
}

pub(super) fn apply_lighting(inst: &Instance, env: &mut Environment) {
	if let Some(Prop::Color3(c)) = inst.prop("OutdoorAmbient") {
		env.ambient_sky = *c;
	}
	if let Some(Prop::Color3(c)) = inst.prop("Ambient") {
		env.ambient_ground = [c[0] * 0.6, c[1] * 0.6, c[2] * 0.6];
	}
	if let Some(Prop::Color3(c)) = inst.prop("FogColor") {
		env.fog_color = *c;
		env.sky_horizon = [c[0].mul_add(0.5, 0.4), c[1].mul_add(0.5, 0.45), c[2].mul_add(0.5, 0.5)];
	}
	if let Some(Prop::F32(end)) = inst.prop("FogEnd") {
		if *end > 50.0 {
			env.fog_density = 2.4 / *end;
		}
	}
	if let Some(Prop::F32(b)) = inst.prop("Brightness") {
		let k = (b / 2.0).clamp(0.3, 2.5);
		env.sun_color = [k, k * 0.98, k * 0.92];
	}
	// ClockTime drives a simple sun arc.
	let time = inst
		.prop("ClockTime")
		.and_then(|p| p.as_f32())
		.or_else(|| inst.prop("TimeOfDay").and_then(|p| p.as_str()).and_then(parse_tod))
		.unwrap_or(14.0);
	let ang = (time - 6.0) / 12.0 * std::f32::consts::PI; // 6:00 sunrise .. 18:00 sunset
	env.sun_dir = [ang.cos() * 0.6, ang.sin().max(0.08), 0.35];
	env.sun_dir = {
		let l = (env.sun_dir[0] * env.sun_dir[0]
			+ env.sun_dir[1] * env.sun_dir[1]
			+ env.sun_dir[2] * env.sun_dir[2])
			.sqrt();
		[env.sun_dir[0] / l, env.sun_dir[1] / l, env.sun_dir[2] / l]
	};
}

fn parse_tod(s: &str) -> Option<f32> {
	let mut it = s.split(':');
	let h: f32 = it.next()?.parse().ok()?;
	let m: f32 = it.next().unwrap_or("0").parse().ok()?;
	Some(h + m / 60.0)
}

/// Convert BodyColors ids into a simple R6 box rig for our own character.
/// Returns (name, pos relative, size, color) parts positioned around `origin`
/// facing -Z (the spawn orientation).
pub fn build_r6_rig(
	origin: Vec3,
	yaw: f32,
	body: &crate::roblox::join::AvatarAppearance,
	out: &mut Vec<ScenePart>,
) -> usize {
	let bc = &body.body_colors;
	let col = |id: u64| -> [f32; 3] {
		if id == 0 {
			[0.63, 0.77, 0.55]
		} else {
			brick_color(id)
		}
	};
	let rot = crate::gfx::math::rot_yaw_pitch(yaw, 0.0);
	let push = |out: &mut Vec<ScenePart>, pos: Vec3, size: Vec3, color: [f32; 3]| {
		out.push(ScenePart {
			pos,
			size,
			rot,
			color: [color[0], color[1], color[2], 1.0],
			roughness: 0.8,
			emissive: 0.0,
			cast_shadow: true,
		});
	};
	let (ox, oy, oz) = (origin[0], origin[1], origin[2]);
	// R6 proportions in studs.
	push(out, [ox, oy + 2.6, oz], [2.0, 1.0, 1.0], col(bc.torso_color_id)); // torso
	push(out, [ox, oy + 3.6, oz], [1.2, 1.2, 1.2], col(bc.head_color_id)); // head
	push(out, [ox - 1.5, oy + 2.6, oz], [1.0, 1.0, 1.0], col(bc.left_arm_color_id));
	push(out, [ox + 1.5, oy + 2.6, oz], [1.0, 1.0, 1.0], col(bc.right_arm_color_id));
	push(out, [ox - 0.5, oy + 0.5, oz], [1.0, 2.0, 1.0], col(bc.left_leg_color_id));
	push(out, [ox + 0.5, oy + 0.5, oz], [1.0, 2.0, 1.0], col(bc.right_leg_color_id));
	6
}

/// Parse a quaternion (x,y,z,w) into a column-major 3x3.
pub fn quat_rot(x: f32, y: f32, z: f32, w: f32) -> [f32; 9] {
	quat_to_mat3([x, y, z, w])
}

/// Fallback ground so the view is always a recognizable place: a
/// Roblox-style green baseplate with a 32-stud grid, a spawn pad and a few
/// landmark blocks. The replicated world draws on top of it.
pub fn build_ground(scene: &mut SceneView) {
	const ID: [f32; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
	let mut push = |pos: Vec3, size: Vec3, color: [f32; 3]| {
		scene.parts.push(ScenePart {
			pos,
			size,
			rot: ID,
			color: [color[0], color[1], color[2], 1.0],
			roughness: 0.95,
			emissive: 0.0,
			cast_shadow: false,
		});
	};
	let grass = [0.40, 0.56, 0.32];
	let line = [0.32, 0.46, 0.26];
	// Baseplate.
	push([0.0, -0.5, 0.0], [2048.0, 1.0, 2048.0], grass);
	// Grid every 32 studs, just above the surface.
	let step = 32.0f32;
	let half = 512.0f32;
	let mut x = -half;
	while x <= half {
		push([x, 0.02, 0.0], [0.40, 0.06, half * 2.0], line);
		x += step;
	}
	let mut z = -half;
	while z <= half {
		push([0.0, 0.02, z], [half * 2.0, 0.06, 0.40], line);
		z += step;
	}
	// Spawn pad.
	push([0.0, 0.08, 0.0], [24.0, 0.2, 24.0], [0.56, 0.66, 0.44]);
	// Landmarks: coloured blocks + a tower, so walking/gravity is visible.
	let marks: [(f32, f32, f32, [f32; 3]); 6] = [
		(-64.0, 0.0, 6.0, [0.75, 0.28, 0.24]),
		(64.0, 0.0, 8.0, [0.24, 0.42, 0.78]),
		(0.0, -64.0, 5.0, [0.80, 0.66, 0.24]),
		(0.0, 64.0, 5.0, [0.42, 0.68, 0.30]),
		(-96.0, 96.0, 26.0, [0.55, 0.55, 0.60]),
		(96.0, -96.0, 3.0, [0.85, 0.45, 0.15]),
	];
	for (mx, mz, h, col) in marks {
		push([mx, h * 0.5, mz], [12.0, h, 12.0], col);
	}
}
