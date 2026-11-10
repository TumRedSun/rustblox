//! High-level 3D scene description handed to the renderer each frame.
//!
//! Everything is built from instanced boxes (Roblox parts ARE boxes,
//! characters are box rigs, terrain voxels merge into boxes) — the GPU side
//! draws them with one instanced call plus a shadow pass.

use crate::gfx::math::Vec3;

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub pos: Vec3,
    /// Radians; yaw around +Y, pitch around the camera right axis.
    pub yaw: f32,
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera { pos: [0.0, 20.0, 40.0], yaw: 0.0, pitch: -0.2, fov: 70.0_f32.to_radians() }
    }
}

impl Camera {
    /// Forward vector (unit).
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        // yaw 0 looks along -Z (like a viewer looking "into" the screen).
        [-sy * cp, sp, -cy * cp]
    }

    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        [cy, 0.0, -sy]
    }
}

/// One renderable box. `rot` is a column-major 3x3 matrix; `size` is the
/// FULL extent (like Roblox's Part.Size), centered at `pos`.
#[derive(Clone, Debug)]
pub struct ScenePart {
    pub pos: Vec3,
    pub size: Vec3,
    pub rot: [f32; 9],
    /// Linear 0..1 color + opacity.
    pub color: [f32; 4],
    /// 0 = mirror, 1 = fully matte.
    pub roughness: f32,
    /// 0 = normal, 1 = unlit neon.
    pub emissive: f32,
    /// Skip the sun shadow for this part (small details).
    pub cast_shadow: bool,
}

/// Lighting/environment parameters (already converted to linear 0..1).
#[derive(Clone, Debug)]
pub struct Environment {
    pub sun_dir: Vec3,
    pub sun_color: [f32; 3],
    pub ambient_sky: [f32; 3],
    pub ambient_ground: [f32; 3],
    pub fog_color: [f32; 3],
    pub fog_density: f32,
    pub sky_top: [f32; 3],
    pub sky_horizon: [f32; 3],
    pub shadow_strength: f32,
}

impl Default for Environment {
    fn default() -> Self {
        Environment {
            sun_dir: [0.4, 0.85, 0.3],
            sun_color: [1.0, 0.97, 0.9],
            ambient_sky: [0.38, 0.42, 0.5],
            ambient_ground: [0.22, 0.2, 0.18],
            fog_color: [0.68, 0.76, 0.88],
            fog_density: 0.0022,
            sky_top: [0.32, 0.55, 0.92],
            sky_horizon: [0.75, 0.84, 0.95],
            shadow_strength: 0.75,
        }
    }
}

impl Environment {
    /// Midday Roblox-style environment.
    pub fn daylight() -> Environment {
        Environment::default()
    }
}

#[derive(Clone, Debug, Default)]
pub struct SceneView {
    pub camera: Camera,
    pub env: Environment,
    pub parts: Vec<ScenePart>,
}

impl SceneView {
    pub fn clear(&mut self) {
        self.parts.clear();
    }
}
