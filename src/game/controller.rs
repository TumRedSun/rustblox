//! Local character + chase-camera controller: WASD/touch movement with
//! gravity and AABB collision, plus the debug fly mode.

use super::Aabb;
use crate::gfx::scene::Camera;

/// Which movement model the local character uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlMode {
	/// Roblox-like: walk on the ground, jump, gravity, collisions (default).
	Walk,
	/// Debug free-fly (F toggles).
	Fly,
}

/// Local character + camera controller.
#[derive(Clone)]
pub struct GameController {
	pub camera: Camera,
	pub pointer_locked: bool,
	pub last_mouse: (f32, f32),
	pub keys: std::collections::HashSet<winit::keyboard::PhysicalKey>,
	/// Touch pad (mobile): joystick deflection (strafe, fwd), each -1..=1.
	pub touch_stick: (f32, f32),
	/// Touch pad (mobile): +1 while the up button is held, -1 down, 0 none.
	pub touch_lift: f32,
	/// Character feet position (walk mode).
	pub pos: crate::gfx::math::Vec3,
	pub vel: crate::gfx::math::Vec3,
	pub on_ground: bool,
	/// Facing yaw of the character.
	pub yaw: f32,
	pub mode: ControlMode,
	/// Pending jump request (consumed by `step`).
	pub jump: bool,
	/// Pending respawn request (R key).
	pub respawn: bool,
	/// True once the character was placed at the spawn point.
	pub spawned: bool,
}

impl Default for GameController {
	fn default() -> Self {
		GameController {
			camera: Camera::default(),
			pointer_locked: false,
			last_mouse: (-1.0, -1.0),
			keys: std::collections::HashSet::new(),
			touch_stick: (0.0, 0.0),
			touch_lift: 0.0,
			pos: [0.0, 0.0, 0.0],
			vel: [0.0, 0.0, 0.0],
			on_ground: false,
			yaw: 0.0,
			mode: ControlMode::Walk,
			jump: false,
			respawn: false,
			spawned: false,
		}
	}
}

/// Roblox R6-ish body dimensions (studs).
const BODY_RADIUS: f32 = 1.1;
const BODY_HEIGHT: f32 = 5.0;
const WALK_SPEED: f32 = 16.0;
const FLY_SPEED: f32 = 30.0;
const GRAVITY: f32 = 196.2;
const JUMP_SPEED: f32 = 50.0;
/// Third-person camera distance behind the character.
const CAM_BACK: f32 = 12.0;

impl GameController {
	/// Place the character (and the camera) at a spawn point.
	pub fn place(&mut self, feet: crate::gfx::math::Vec3) {
		self.pos = feet;
		self.vel = [0.0; 3];
		self.on_ground = false;
		self.spawned = true;
		self.update_camera();
	}

	pub fn toggle_fly(&mut self) {
		self.mode = match self.mode {
			ControlMode::Walk => ControlMode::Fly,
			ControlMode::Fly => ControlMode::Walk,
		};
		self.vel = [0.0; 3];
		if self.mode == ControlMode::Walk {
			// Drop out of the air onto whatever is below.
			self.on_ground = false;
		}
	}

	fn held(&self, code: winit::keyboard::KeyCode) -> bool {
		self.keys.contains(&winit::keyboard::PhysicalKey::Code(code))
	}

	/// Wish direction on the ground plane from WASD + the touch stick,
	/// relative to where the camera looks.
	fn wish_dir(&self, axes: (f32, f32)) -> (f32, f32) {
		let fwd = self.camera.forward();
		let flat = (fwd[0] * fwd[0] + fwd[2] * fwd[2]).sqrt();
		let (fx, fz) = if flat > 1e-4 { (fwd[0] / flat, fwd[2] / flat) } else { (0.0, -1.0) };
		let right = self.camera.right();
		let (ix, iz) = axes;
		let x = fx * iz + right[0] * ix;
		let z = fz * iz + right[2] * ix;
		let len = (x * x + z * z).sqrt();
		if len > 1e-4 {
			(x / len, z / len)
		} else {
			(0.0, 0.0)
		}
	}

	fn axes(&self) -> (f32, f32) {
		use winit::keyboard::KeyCode;
		let mut ix = 0.0;
		let mut iz = 0.0;
		if self.held(KeyCode::KeyW) || self.held(KeyCode::ArrowUp) {
			iz += 1.0;
		}
		if self.held(KeyCode::KeyS) || self.held(KeyCode::ArrowDown) {
			iz -= 1.0;
		}
		if self.held(KeyCode::KeyD) || self.held(KeyCode::ArrowRight) {
			ix += 1.0;
		}
		if self.held(KeyCode::KeyA) || self.held(KeyCode::ArrowLeft) {
			ix -= 1.0;
		}
		let (sx, sy) = self.touch_stick;
		ix += sx;
		iz += sy;
		let len = (ix * ix + iz * iz).sqrt();
		if len > 1.0 {
			(ix / len, iz / len)
		} else {
			(ix, iz)
		}
	}

	/// Advance the character / camera by `dt`. `colliders` are the world-space
	/// boxes of the decoded scene (empty before replication arrives).
	pub fn step(&mut self, dt: f32, colliders: &[Aabb]) {
		let axes = self.axes();
		match self.mode {
			ControlMode::Fly => self.step_fly(dt, axes),
			ControlMode::Walk => self.step_walk(dt, axes, colliders),
		}
		self.update_camera();
	}

	fn step_fly(&mut self, dt: f32, axes: (f32, f32)) {
		use winit::keyboard::KeyCode;
		let fwd = self.camera.forward();
		let right = self.camera.right();
		let mut delta = crate::gfx::math::add3(
			crate::gfx::math::scale3(fwd, axes.1),
			crate::gfx::math::scale3(right, axes.0),
		);
		if self.held(KeyCode::Space) {
			delta[1] += 1.0;
		}
		if self.held(KeyCode::ShiftLeft) || self.held(KeyCode::ShiftRight) {
			delta[1] -= 1.0;
		}
		if self.touch_lift.abs() > 1e-3 {
			delta[1] += self.touch_lift;
		}
		let len = crate::gfx::math::len3(delta);
		if len > 1e-4 {
			let d = crate::gfx::math::scale3(delta, FLY_SPEED * dt / len);
			self.camera.pos = crate::gfx::math::add3(self.camera.pos, d);
		}
	}

	fn step_walk(&mut self, dt: f32, axes: (f32, f32), colliders: &[Aabb]) {
		let (wx, wz) = self.wish_dir(axes);
		let want = [wx * WALK_SPEED, 0.0, wz * WALK_SPEED];
		// Snappy ground control: blend the horizontal velocity toward the wish.
		let blend = (dt * 14.0).min(1.0);
		self.vel[0] += (want[0] - self.vel[0]) * blend;
		self.vel[2] += (want[2] - self.vel[2]) * blend;
		if (wx * wx + wz * wz).sqrt() > 1e-4 {
			self.yaw = wx.atan2(wz);
		}

		if self.on_ground && self.jump {
			self.vel[1] = JUMP_SPEED;
			self.on_ground = false;
		}
		self.jump = false;
		self.vel[1] -= GRAVITY * dt;
		if self.vel[1] < -200.0 {
			self.vel[1] = -200.0;
		}

		let prev_y = self.pos[1];
		self.pos[0] += self.vel[0] * dt;
		self.resolve_horizontal(colliders, 0);
		self.pos[2] += self.vel[2] * dt;
		self.resolve_horizontal(colliders, 2);
		self.pos[1] += self.vel[1] * dt;
		self.resolve_vertical(colliders, prev_y);

		// Safety net: never fall through the world (the rendered baseplate
		// sits at y = 0 until the server's ground is decoded).
		if self.pos[1] < 0.0 {
			self.pos[1] = 0.0;
			if self.vel[1] < 0.0 {
				self.vel[1] = 0.0;
				self.on_ground = true;
			}
		}
		if self.respawn {
			self.respawn = false;
			if self.pos[1] < 0.0 {
				self.pos[1] = 8.0;
			}
		}
	}

	fn overlaps(&self, c: &Aabb) -> bool {
		let p = self.pos;
		p[0] + BODY_RADIUS > c.min[0]
			&& p[0] - BODY_RADIUS < c.max[0]
			&& p[1] + BODY_HEIGHT > c.min[1]
			&& p[1] < c.max[1]
			&& p[2] + BODY_RADIUS > c.min[2]
			&& p[2] - BODY_RADIUS < c.max[2]
	}

	fn resolve_horizontal(&mut self, colliders: &[Aabb], axis: usize) {
		for c in colliders {
			if !self.overlaps(c) {
				continue;
			}
			// Ignore boxes we are standing on top of.
			if self.pos[1] >= c.max[1] - 0.05 {
				continue;
			}
			if self.vel[axis] > 0.0 {
				self.pos[axis] = c.min[axis] - BODY_RADIUS - 1e-3;
			} else if self.vel[axis] < 0.0 {
				self.pos[axis] = c.max[axis] + BODY_RADIUS + 1e-3;
			}
			self.vel[axis] = 0.0;
		}
	}

	fn resolve_vertical(&mut self, colliders: &[Aabb], prev_y: f32) {
		let mut landed = false;
		for c in colliders {
			if self.pos[0] + BODY_RADIUS <= c.min[0]
				|| self.pos[0] - BODY_RADIUS >= c.max[0]
				|| self.pos[2] + BODY_RADIUS <= c.min[2]
				|| self.pos[2] - BODY_RADIUS >= c.max[2]
			{
				continue;
			}
			if self.vel[1] <= 0.0
				&& prev_y >= c.max[1] - 0.35
				&& self.pos[1] <= c.max[1]
				&& self.pos[1] + BODY_HEIGHT > c.min[1]
			{
				self.pos[1] = c.max[1];
				landed = true;
				continue;
			}
			if self.vel[1] > 0.0 && self.overlaps(c) {
				self.pos[1] = c.min[1] - BODY_HEIGHT - 1e-3;
				self.vel[1] = 0.0;
			}
		}
		if landed {
			self.vel[1] = 0.0;
			self.on_ground = true;
		} else if self.vel[1].abs() > 0.001 {
			self.on_ground = false;
		}
	}

	/// Chase camera: sit behind the character along the look direction.
	fn update_camera(&mut self) {
		if self.mode == ControlMode::Fly {
			return;
		}
		let fwd = self.camera.forward();
		let eye = [self.pos[0], self.pos[1] + 4.6, self.pos[2]];
		let mut cam =
			[eye[0] - fwd[0] * CAM_BACK, eye[1] - fwd[1] * CAM_BACK, eye[2] - fwd[2] * CAM_BACK];
		// Never dip below the character's feet.
		if cam[1] < self.pos[1] + 0.8 {
			cam[1] = self.pos[1] + 0.8;
		}
		self.camera.pos = cam;
	}

	pub fn mouse_delta(&mut self, x: f32, y: f32) {
		if !self.pointer_locked {
			self.last_mouse = (x, y);
			return;
		}
		if self.last_mouse.0 < 0.0 {
			self.last_mouse = (x, y);
			return;
		}
		let dx = x - self.last_mouse.0;
		let dy = y - self.last_mouse.1;
		self.last_mouse = (x, y);
		self.camera.yaw -= dx * 0.0028;
		self.camera.pitch = (self.camera.pitch - dy * 0.0028).clamp(-1.35, 1.1);
	}

	/// Camera look from a touch drag (mobile game pad): the finger delta
	/// rotates the camera with the same sensitivity as the locked mouse.
	pub fn touch_look(&mut self, dx: f32, dy: f32) {
		self.camera.yaw -= dx * 0.0028;
		self.camera.pitch = (self.camera.pitch - dy * 0.0028).clamp(-1.35, 1.1);
	}
}
