//! 3D scene pipeline state: frame uniforms and instance packing.

use super::Gfx;
use crate::gfx::math::{self, Mat4};
use crate::gfx::scene::{Camera, Environment, SceneView};
use bytemuck::{Pod, Zeroable};

mod pipeline;
pub(crate) use pipeline::SceneResources;

/// One scene instance (must match the WGSL vertex layout).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct SceneInstance {
	rot: [f32; 9],    // column-major 3x3 at 0..36
	_pad0: [f32; 3],  // 36..48
	pos: [f32; 4],    // 48..64
	size: [f32; 4],   // 64..80
	color: [f32; 4],  // 80..96
	params: [f32; 4], // 96..112
}

/// Per-frame scene uniform block (camera, shadow matrix, lighting).
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct FrameUniforms {
	view_proj: [f32; 16],
	inv_view_proj: [f32; 16],
	light_view_proj: [f32; 16],
	cam_pos: [f32; 4],
	sun_dir: [f32; 4],
	sun_color: [f32; 4],
	ambient_sky: [f32; 4],
	ambient_ground: [f32; 4],
	fog_color: [f32; 4],
	sky_top: [f32; 4],
	sky_horizon: [f32; 4],
	params: [f32; 4],
}

/// Instance-buffer growth starts here and doubles as needed.
const MAX_SCENE_INSTANCES: usize = 1 << 18;

impl Gfx {
	pub(super) fn build_frame_uniforms(&self, cam: &Camera, env: &Environment) -> FrameUniforms {
		let aspect = self.config.width as f32 / self.config.height as f32;
		let eye = cam.pos;
		let fwd = cam.forward();
		let target = [eye[0] + fwd[0], eye[1] + fwd[1], eye[2] + fwd[2]];
		let view_proj = Mat4::mul(
			&Mat4::perspective(cam.fov, aspect, 0.05, 1200.0),
			&Mat4::look_at(eye, target, [0.0, 1.0, 0.0]),
		);
		let sun = math::norm3(env.sun_dir);
		// Shadow camera: ortho box around the viewer.
		let radius = 180.0_f32;
		let light_eye = [eye[0] + sun[0] * 300.0, eye[1] + sun[1] * 300.0, eye[2] + sun[2] * 300.0];
		let light_view = Mat4::look_at(light_eye, eye, [0.0, 1.0, 0.0]);
		let light_view_proj =
			Mat4::mul(&Mat4::ortho(-radius, radius, -radius, radius, 20.0, 700.0), &light_view);
		FrameUniforms {
			view_proj: view_proj.0,
			inv_view_proj: view_proj.inverse().0,
			light_view_proj: light_view_proj.0,
			cam_pos: [eye[0], eye[1], eye[2], 1.0],
			sun_dir: [sun[0], sun[1], sun[2], 0.0],
			// Gamma-space lighting throughout (consistent with the UI path
			// on a non-sRGB surface): colors pass through unchanged.
			sun_color: [env.sun_color[0], env.sun_color[1], env.sun_color[2], 1.0],
			ambient_sky: [env.ambient_sky[0], env.ambient_sky[1], env.ambient_sky[2], 1.0],
			ambient_ground: [
				env.ambient_ground[0],
				env.ambient_ground[1],
				env.ambient_ground[2],
				1.0,
			],
			fog_color: [env.fog_color[0], env.fog_color[1], env.fog_color[2], env.fog_density],
			sky_top: [env.sky_top[0], env.sky_top[1], env.sky_top[2], 1.0],
			sky_horizon: [env.sky_horizon[0], env.sky_horizon[1], env.sky_horizon[2], 1.0],
			params: [0.0, env.shadow_strength, 0.0, 0.0],
		}
	}

	/// Convert scene parts into GPU instances and grow the vertex buffer when
	/// the frame needs more room.
	pub(super) fn pack_scene(&mut self, scene: &SceneView) {
		self.scratch_scene.clear();
		for p in &scene.parts {
			self.scratch_scene.push(SceneInstance {
				rot: p.rot,
				_pad0: [0.0; 3],
				pos: [p.pos[0], p.pos[1], p.pos[2], 0.0],
				size: [p.size[0], p.size[1], p.size[2], 0.0],
				color: p.color,
				params: [p.roughness, p.emissive, 0.0, 0.0],
			});
		}
		self.ensure_scene_capacity();
	}

	fn ensure_scene_capacity(&mut self) {
		if self.scratch_scene.len() > self.scene.capacity {
			let mut n = self.scene.capacity;
			while n < self.scratch_scene.len() {
				n *= 2;
			}
			self.scene.instances = super::make_buf(
				&self.device,
				n,
				std::mem::size_of::<SceneInstance>(),
				"scene-instances",
			);
			self.scene.capacity = n;
		}
	}
}
