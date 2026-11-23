//! World queries used by the game thread: spawn point and character
//! colliders decoded from the DataModel.

use crate::game::Aabb;
use crate::gfx::math::Vec3;
use crate::world::{DataModel, Prop};

/// First SpawnLocation/SpawnPart position in the world.
pub(super) fn find_spawn(dm: &DataModel) -> Option<Vec3> {
	let root = dm.find_root("DataModel").or(dm.root)?;
	for id in dm.walk(root) {
		let Some(inst) = dm.get(id) else { continue };
		if matches!(inst.class.as_str(), "SpawnLocation" | "SpawnPart") {
			let pos = match inst.prop("CFrame") {
				Some(Prop::CFrame { pos, .. }) => Some(*pos),
				_ => inst.prop("Position").and_then(|p| match p {
					Prop::Vec3(v) => Some(*v),
					_ => None,
				}),
			};
			if let Some(mut p) = pos {
				p[1] += 4.0;
				return Some(p);
			}
		}
	}
	None
}

/// Colliders for the local character: every solid part of the decoded world
/// as a world-space AABB (rotation approximated by its extents, which is
/// exact for the axis-aligned parts that dominate Roblox builds).
pub(super) fn collect_colliders(dm: &DataModel) -> Vec<Aabb> {
	let Some(root) = dm.find_root("DataModel").or(dm.root) else { return Vec::new() };
	let mut out = Vec::new();
	for id in dm.walk(root) {
		let Some(inst) = dm.get(id) else { continue };
		if !matches!(
			inst.class.as_str(),
			"Part"
				| "WedgePart"
				| "CornerWedgePart"
				| "MeshPart"
				| "TrussPart"
				| "SpawnLocation"
				| "UnionOperation"
		) {
			continue;
		}
		let (pos, size) = match (inst.prop("CFrame"), inst.prop("Size")) {
			(Some(Prop::CFrame { pos, .. }), Some(Prop::Vec3(s))) => (pos, s),
			_ => continue,
		};
		let rot = match inst.prop("CFrame") {
			Some(Prop::CFrame { rot, .. }) => *rot,
			_ => [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
		};
		// Column-major basis: columns are rot[0..3], rot[3..6], rot[6..9].
		let half = [
			(rot[0].abs() * size[0] + rot[3].abs() * size[1] + rot[6].abs() * size[2]) * 0.5,
			(rot[1].abs() * size[0] + rot[4].abs() * size[1] + rot[7].abs() * size[2]) * 0.5,
			(rot[2].abs() * size[0] + rot[5].abs() * size[1] + rot[8].abs() * size[2]) * 0.5,
		];
		if !pos.iter().all(|v| v.is_finite()) || !half.iter().all(|v| v.is_finite()) {
			continue;
		}
		out.push(Aabb::centre_half(*pos, half));
		if out.len() >= 20_000 {
			break;
		}
	}
	out
}
