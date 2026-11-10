//! Minimal column-major 4x4 matrix + vec3 math for the renderer.
//!
//! Deliberately dependency-free: only what the scene pipeline needs
//! (perspective / ortho / look-at / multiply / inverse, rotations, normalize).

pub type Vec3 = [f32; 3];
pub type Quat = [f32; 4]; // xyzw

/// Column-major 4x4 (m[col * 4 + row]) — matches WGSL `mat4x4<f32>` layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4([
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]);

    pub fn zero() -> Mat4 {
        Mat4([0.0; 16])
    }

    /// a * b (apply b first, then a).
    pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
        let mut o = [0.0f32; 16];
        for c in 0..4 {
            for r in 0..4 {
                let mut s = 0.0;
                for k in 0..4 {
                    s += a.0[k * 4 + r] * b.0[c * 4 + k];
                }
                o[c * 4 + r] = s;
            }
        }
        Mat4(o)
    }

    /// Transform a point (w=1).
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        [
            self.0[0] * p[0] + self.0[4] * p[1] + self.0[8] * p[2] + self.0[12],
            self.0[1] * p[0] + self.0[5] * p[1] + self.0[9] * p[2] + self.0[13],
            self.0[2] * p[0] + self.0[6] * p[1] + self.0[10] * p[2] + self.0[14],
        ]
    }

    /// Transform a direction (w=0).
    pub fn transform_dir(&self, d: Vec3) -> Vec3 {
        [
            self.0[0] * d[0] + self.0[4] * d[1] + self.0[8] * d[2],
            self.0[1] * d[0] + self.0[5] * d[1] + self.0[9] * d[2],
            self.0[2] * d[0] + self.0[6] * d[1] + self.0[10] * d[2],
        ]
    }

    /// Right-handed perspective projection with Vulkan depth range [0, 1].
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        let f = 1.0 / (fov_y * 0.5).tan();
        let nf = 1.0 / (near - far);
        Mat4([
            f / aspect, 0.0, 0.0, 0.0, //
            0.0, f, 0.0, 0.0, //
            0.0, 0.0, far * nf, -1.0, //
            0.0, 0.0, far * near * nf, 0.0,
        ])
    }

    /// Right-handed orthographic projection, depth range [0, 1].
    pub fn ortho(l: f32, r: f32, b: f32, t: f32, n: f32, f: f32) -> Mat4 {
        let nf = 1.0 / (n - f);
        Mat4([
            2.0 / (r - l), 0.0, 0.0, 0.0, //
            0.0, 2.0 / (t - b), 0.0, 0.0, //
            0.0, 0.0, nf, 0.0, //
            -(r + l) / (r - l), -(t + b) / (t - b), n * nf, 1.0,
        ])
    }

    /// Right-handed look-at (camera looks along -Z).
    pub fn look_at(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
        let f = norm3(sub3(center, eye)); // forward
        let s = norm3(cross3(f, up)); // right
        let u = cross3(s, f); // true up
        Mat4([
            s[0], u[0], -f[0], 0.0, //
            s[1], u[1], -f[1], 0.0, //
            s[2], u[2], -f[2], 0.0, //
            -dot3(s, eye), -dot3(u, eye), dot3(f, eye), 1.0,
        ])
    }

    /// General 4x4 inverse (cofactor method). Returns zero matrix for
    /// singular input.
    pub fn inverse(&self) -> Mat4 {
        let m = &self.0;
        let a00 = m[0]; let a01 = m[1]; let a02 = m[2]; let a03 = m[3];
        let a10 = m[4]; let a11 = m[5]; let a12 = m[6]; let a13 = m[7];
        let a20 = m[8]; let a21 = m[9]; let a22 = m[10]; let a23 = m[11];
        let a30 = m[12]; let a31 = m[13]; let a32 = m[14]; let a33 = m[15];

        let b00 = a00 * a11 - a01 * a10;
        let b01 = a00 * a12 - a02 * a10;
        let b02 = a00 * a13 - a03 * a10;
        let b03 = a01 * a12 - a02 * a11;
        let b04 = a01 * a13 - a03 * a11;
        let b05 = a02 * a13 - a03 * a12;
        let b06 = a20 * a31 - a21 * a30;
        let b07 = a20 * a32 - a22 * a30;
        let b08 = a20 * a33 - a23 * a30;
        let b09 = a21 * a32 - a22 * a31;
        let b10 = a21 * a33 - a23 * a31;
        let b11 = a22 * a33 - a23 * a32;

        let det = b00 * b11 - b01 * b10 + b02 * b09 + b03 * b08 - b04 * b07 + b05 * b06;
        if det.abs() < 1e-12 {
            return Mat4::zero();
        }
        let inv = 1.0 / det;
        Mat4([
            (a11 * b11 - a12 * b10 + a13 * b09) * inv,
            (a02 * b10 - a01 * b11 - a03 * b09) * inv,
            (a31 * b05 - a32 * b04 + a33 * b03) * inv,
            (a22 * b04 - a21 * b05 - a23 * b03) * inv,
            (a12 * b08 - a10 * b11 - a13 * b07) * inv,
            (a00 * b11 - a02 * b08 + a03 * b07) * inv,
            (a32 * b02 - a30 * b05 - a33 * b01) * inv,
            (a20 * b05 - a22 * b02 + a23 * b01) * inv,
            (a10 * b10 - a11 * b08 + a13 * b06) * inv,
            (a01 * b08 - a00 * b10 - a03 * b06) * inv,
            (a30 * b04 - a31 * b02 + a33 * b00) * inv,
            (a21 * b02 - a20 * b04 - a23 * b00) * inv,
            (a11 * b07 - a10 * b09 - a12 * b06) * inv,
            (a00 * b09 - a01 * b07 + a02 * b06) * inv,
            (a31 * b01 - a30 * b03 - a32 * b00) * inv,
            (a20 * b03 - a21 * b01 + a22 * b00) * inv,
        ])
    }

    /// Build from a 3x3 rotation (column-major, 9 floats) + translation.
    pub fn from_rot_pos(rot: &[f32; 9], pos: Vec3) -> Mat4 {
        let mut m = Mat4::IDENTITY;
        for c in 0..3 {
            for r in 0..3 {
                m.0[c * 4 + r] = rot[c * 3 + r];
            }
        }
        m.0[12] = pos[0];
        m.0[13] = pos[1];
        m.0[14] = pos[2];
        m
    }
}

pub fn sub3(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn add3(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn scale3(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

pub fn dot3(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn cross3(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn len3(a: Vec3) -> f32 {
    dot3(a, a).sqrt()
}

pub fn norm3(a: Vec3) -> Vec3 {
    let l = len3(a);
    if l > 1e-8 { scale3(a, 1.0 / l) } else { [0.0, 0.0, 0.0] }
}

/// Rotation matrix (column-major 3x3) from yaw (Y) then pitch (X).
pub fn rot_yaw_pitch(yaw: f32, pitch: f32) -> [f32; 9] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    // R = Ry(yaw) * Rx(pitch), column-major
    [
        cy, 0.0, -sy, //
        sy * sp, cp, cy * sp, //
        sy * cp, -sp, cy * cp,
    ]
}

/// Rotation matrix (column-major 3x3) from a unit quaternion.
pub fn quat_to_mat3(q: Quat) -> [f32; 9] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    let x2 = x + x; let y2 = y + y; let z2 = z + z;
    let xx = x * x2; let xy = x * y2; let xz = x * z2;
    let yy = y * y2; let yz = y * z2; let zz = z * z2;
    let wx = w * x2; let wy = w * y2; let wz = w * z2;
    [
        1.0 - (yy + zz), xy + wz, xz - wy, //
        xy - wz, 1.0 - (xx + zz), yz + wx, //
        xz + wy, yz - wx, 1.0 - (xx + yy),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perspective_depth_range() {
        let p = Mat4::perspective(1.0, 1.0, 0.1, 100.0);
        let near = p.transform_point([0.0, 0.0, -0.1]);
        let far = p.transform_point([0.0, 0.0, -100.0]);
        let zn = near[2];
        let zf = far[2];
        let wn = 0.1;  // clip w = -view_z
        let wf = 100.0;
        assert!((zn / wn).abs() < 1e-5, "near depth {} should be 0", zn / wn);
        assert!((zf / wf - 1.0).abs() < 1e-5, "far depth {} should be 1", zf / wf);
    }

    #[test]
    fn look_at_identity_basis() {
        let v = Mat4::look_at([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        // Camera at +Z looking at origin: origin maps to view (0,0,-5).
        let o = v.transform_point([0.0, 0.0, 0.0]);
        assert!((o[0]).abs() < 1e-6 && (o[1]).abs() < 1e-6 && (o[2] + 5.0).abs() < 1e-6);
    }

    #[test]
    fn inverse_roundtrip() {
        let m = Mat4::look_at([1.0, 2.0, 3.0], [-4.0, 0.5, 7.0], [0.0, 1.0, 0.0]);
        let r = Mat4::mul(&m, &m.inverse());
        for i in 0..16 {
            let want = if i % 5 == 0 { 1.0 } else { 0.0 };
            assert!((r.0[i] - want).abs() < 1e-4);
        }
    }

    #[test]
    fn quat_is_rotation() {
        let q: Quat = {
            let half = 0.5_f32 * std::f32::consts::FRAC_PI_2;
            [0.0, half.sin(), 0.0, half.cos()]
        };
        let m = quat_to_mat3(q);
        // Rotate +X by +90° yaw (right-handed): +X -> -Z.
        let v = [m[0], m[1], m[2]];
        assert!((v[0]).abs() < 1e-5 && (v[2].abs() - 1.0).abs() < 1e-5);
    }
}
