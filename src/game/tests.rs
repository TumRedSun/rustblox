//! Character controller tests: gravity, WASD, jump, collision and fly mode.

use crate::game::{Aabb, GameController};
use winit::keyboard::{KeyCode, PhysicalKey};

fn walker() -> GameController {
	let mut c = GameController::default();
	c.place([0.0, 0.0, 0.0]);
	c
}

#[test]
fn character_falls_and_lands() {
	let mut c = walker();
	c.pos = [0.0, 10.0, 0.0];
	for _ in 0..240 {
		c.step(1.0 / 60.0, &[]);
	}
	assert!(c.on_ground, "character should be standing");
	assert!(c.pos[1].abs() < 0.01, "feet at y=0, got {}", c.pos[1]);
}

#[test]
fn wasd_moves_and_faces_the_move_direction() {
	let mut c = walker();
	c.keys.insert(PhysicalKey::Code(KeyCode::KeyW));
	for _ in 0..60 {
		c.step(1.0 / 60.0, &[]);
	}
	// yaw 0 looks along -Z, so W walks toward -Z.
	assert!(c.pos[2] < -5.0, "walked along -Z, z={}", c.pos[2]);
	assert!(c.pos[0].abs() < 0.5);
}

#[test]
fn space_jumps() {
	let mut c = walker();
	for _ in 0..30 {
		c.step(1.0 / 60.0, &[]);
	}
	assert!(c.on_ground);
	c.jump = true;
	c.step(1.0 / 60.0, &[]);
	assert!(c.vel[1] > 10.0, "jump impulse applied, vy={}", c.vel[1]);
	assert!(!c.on_ground);
}

#[test]
fn collider_blocks_walking() {
	let mut c = walker();
	// Wall spanning x at z = -10.
	let wall = Aabb::centre_half([0.0, 5.0, -10.0], [20.0, 5.0, 1.0]);
	c.keys.insert(PhysicalKey::Code(KeyCode::KeyW));
	for _ in 0..180 {
		c.step(1.0 / 60.0, &[wall]);
	}
	// Stopped in front of the wall (its face is at z = -9).
	assert!(c.pos[2] > -9.5, "blocked by the wall, z={}", c.pos[2]);
}

#[test]
fn stands_on_a_collider_top() {
	let mut c = walker();
	let platform = Aabb::centre_half([0.0, 5.0, 0.0], [10.0, 5.0, 10.0]);
	c.pos = [0.0, 15.0, 0.0];
	for _ in 0..240 {
		c.step(1.0 / 60.0, &[platform]);
	}
	assert!(c.on_ground);
	assert!((c.pos[1] - 10.0).abs() < 0.05, "standing on the box top, y={}", c.pos[1]);
}

#[test]
fn fly_mode_ignores_gravity() {
	let mut c = walker();
	c.toggle_fly();
	c.keys.insert(PhysicalKey::Code(KeyCode::Space));
	let y0 = c.camera.pos[1];
	for _ in 0..30 {
		c.step(1.0 / 60.0, &[]);
	}
	assert!(c.camera.pos[1] > y0 + 5.0, "flew up: {} -> {}", y0, c.camera.pos[1]);
}
