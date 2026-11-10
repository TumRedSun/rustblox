// Scene pipeline: instanced cube parts with lighting, shadows, sky and fog.
// Entries:
//   vs_shadow / fs_shadow — depth-only shadow map pass
//   vs_scene  / fs_scene  — lit geometry pass
//   vs_sky    / fs_sky    — fullscreen sky gradient + sun disc

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    cam_pos: vec4<f32>,
    sun_dir: vec4<f32>,      // xyz = direction TO the sun (normalized)
    sun_color: vec4<f32>,    // rgb, a unused
    ambient_sky: vec4<f32>,  // rgb
    ambient_ground: vec4<f32>,
    fog_color: vec4<f32>,    // rgb, a = density
    sky_top: vec4<f32>,      // rgb
    sky_horizon: vec4<f32>,  // rgb
    params: vec4<f32>,       // x = srgb surface, y = shadow strength, z = sun disc size, w = time
};

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var shadow_map: texture_depth_2d;
@group(1) @binding(1) var shadow_samp: sampler_comparison;

// Vertex inputs for the scene/shadow passes: builtins plus per-part
// instance data. A mat3x3 cannot be a vertex attribute directly, so the
// rotation arrives as three column vectors at locations 0..2 (this matches
// the Rust-side gfx::SceneInstance layout). Location 3 is intentionally
// unused by both sides.
struct SceneIn {
    @builtin(vertex_index) vi: u32,
    @location(0) rc0: vec3<f32>,        // rotation column 0
    @location(1) rc1: vec3<f32>,        // rotation column 1
    @location(2) rc2: vec3<f32>,        // rotation column 2
    @location(4) pos: vec4<f32>,        // xyz, w unused
    @location(5) size: vec4<f32>,       // full extents, w unused
    @location(6) color: vec4<f32>,      // rgba (a = opacity)
    @location(7) params: vec4<f32>,     // x = roughness, y = emissive, z/w unused
};

fn instance_rot(in: SceneIn) -> mat3x3<f32> {
    return mat3x3<f32>(in.rc0, in.rc1, in.rc2);
}

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec2<f32>,
};

// 36-vertex cube: 6 faces x 6 vertices (two triangles per face, no index
// buffer). face = vi / 6, k = vi % 6.
// Faces: 0:+X, 1:-X, 2:+Y, 3:-Y, 4:+Z, 5:-Z.
fn cube_pos_normal(vi: u32) -> vec3<f32> {
    let face = vi / 6u;
    let k = vi % 6u;
    let a = face / 2u;            // axis 0..2
    let sgn = select(-1.0, 1.0, face % 2u == 0u);
    let ua = (a + 1u) % 3u;       // tangent axis
    let va = (a + 2u) % 3u;       // bitangent axis
    // Corner pattern for the two triangles of each face:
    // (-1,-1) (1,-1) (1,1) | (1,1) (-1,1) (-1,-1)
    var cu = -1.0;
    var cv = -1.0;
    if (k == 1u || k == 2u || k == 3u) { cu = 1.0; }
    if (k == 2u || k == 3u || k == 4u) { cv = 1.0; }
    var p = vec3<f32>(0.0);
    p[a] = sgn;
    p[ua] = cu;
    p[va] = cv;
    return p;
}

@vertex
fn vs_scene(in: SceneIn) -> VsOut {
    var out: VsOut;
    let rot = instance_rot(in);
    let corner = cube_pos_normal(in.vi);
    let local = corner * in.size.xyz * 0.5;
    let world = in.pos.xyz + rot * local;
    out.world = world;
    out.normal = normalize(rot * cube_normal(in.vi));
    out.clip = frame.view_proj * vec4<f32>(world, 1.0);
    out.color = in.color;
    out.params = in.params.xy;
    return out;
}

fn cube_normal(vi: u32) -> vec3<f32> {
    let face = vi / 6u;
    var n = vec3<f32>(0.0, 0.0, 1.0);
    if (face == 0u) { n = vec3<f32>(1.0, 0.0, 0.0); }
    else if (face == 1u) { n = vec3<f32>(-1.0, 0.0, 0.0); }
    else if (face == 2u) { n = vec3<f32>(0.0, 1.0, 0.0); }
    else if (face == 3u) { n = vec3<f32>(0.0, -1.0, 0.0); }
    else if (face == 4u) { n = vec3<f32>(0.0, 0.0, 1.0); }
    else { n = vec3<f32>(0.0, 0.0, -1.0); }
    return n;
}

fn srgb_to_linear(c: f32) -> f32 {
    return select(c / 12.92, pow((c + 0.055) / 1.055, 2.4), c > 0.04045);
}

fn srgb3(c: vec3<f32>) -> vec3<f32> {
    if (frame.params.x > 0.5) {
        return vec3<f32>(srgb_to_linear(c.r), srgb_to_linear(c.g), srgb_to_linear(c.b));
    }
    return c;
}

fn shadow_factor(world: vec3<f32>, n: vec3<f32>) -> f32 {
    let sp = frame.light_view_proj * vec4<f32>(world + n * 0.05, 1.0);
    let proj = sp.xyz / sp.w;
    let uv = vec2<f32>(proj.x * 0.5 + 0.5, 0.5 - proj.y * 0.5);
    // No early return: textureSampleCompare needs uniform control flow, and
    // a non-uniform return would diverge the quad. Mask the result instead.
    let inside = (uv.x >= 0.0) & (uv.x <= 1.0) & (uv.y >= 0.0) & (uv.y <= 1.0) & (proj.z <= 1.0);
    var sum = 0.0;
    for (var dy: i32 = -1; dy <= 1; dy = dy + 1) {
        for (var dx: i32 = -1; dx <= 1; dx = dx + 1) {
            let o = vec2<f32>(f32(dx), f32(dy)) * (1.0 / 2048.0);
            sum = sum + textureSampleCompare(shadow_map, shadow_samp, uv + o, proj.z - 0.0015);
        }
    }
    return select(1.0, sum / 9.0, inside);
}

@fragment
fn fs_scene(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let l = normalize(frame.sun_dir.xyz);
    let v = normalize(frame.cam_pos.xyz - in.world);
    let h = normalize(l + v);

    let albedo = srgb3(in.color.rgb);
    let opacity = in.color.a;

    let shadow = mix(1.0, shadow_factor(in.world, n), frame.params.y);
    let ndl = max(dot(n, l), 0.0);

    // Hemispheric ambient: sky above, ground below.
    let up_mix = n.y * 0.5 + 0.5;
    let ambient = mix(frame.ambient_ground.rgb, frame.ambient_sky.rgb, up_mix);

    // Blinn-Phong-ish specular; roughness 0..1 mapped to a wide->tight lobe.
    let rough = clamp(in.params.x, 0.02, 1.0);
    let shininess = 2.0 / (rough * rough * rough * rough + 0.004);
    let spec_pow = pow(max(dot(n, h), 0.0), shininess);
    let spec = spec_pow * (1.0 - rough) * 0.5 * shadow;

    var lit = albedo * (ambient + frame.sun_color.rgb * ndl * shadow) + frame.sun_color.rgb * spec;

    // Emissive parts (Neon) skip lighting.
    let emissive = clamp(in.params.y, 0.0, 1.0);
    lit = mix(lit, albedo * (1.0 + emissive * 2.0), emissive);

    // Exponential squared fog toward the horizon color.
    let dist = length(frame.cam_pos.xyz - in.world);
    let fog_d = frame.fog_color.a;
    let fog_amt = 1.0 - exp(-fog_d * fog_d * dist * dist);
    let fogged = mix(lit, srgb3(frame.fog_color.rgb), clamp(fog_amt, 0.0, 1.0));

    return vec4<f32>(fogged, opacity);
}

// ---------------------------------------------------------------------------
// Shadow pass (depth only)
// ---------------------------------------------------------------------------

struct ShadowOut {
    @builtin(position) clip: vec4<f32>,
};

@vertex
fn vs_shadow(in: SceneIn) -> ShadowOut {
    var out: ShadowOut;
    let rot = instance_rot(in);
    let corner = cube_pos_normal(in.vi);
    let local = corner * in.size.xyz * 0.5;
    let world = in.pos.xyz + rot * local;
    out.clip = frame.light_view_proj * vec4<f32>(world, 1.0);
    return out;
}

@fragment
fn fs_shadow() {
    // Depth-only pass: nothing to write.
}

// ---------------------------------------------------------------------------
// Sky (fullscreen triangle)
// ---------------------------------------------------------------------------

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) dir: vec3<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    var out: SkyOut;
    let p = corners[vi];
    out.clip = vec4<f32>(p, 0.99999, 1.0);
    // Reconstruct the world ray from the inverse view-projection.
    let far = frame.inv_view_proj * vec4<f32>(p, 1.0, 1.0);
    let near = frame.inv_view_proj * vec4<f32>(p, 0.0, 1.0);
    out.dir = normalize(far.xyz / far.w - near.xyz / near.w);
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let d = normalize(in.dir);
    let up = clamp(d.y * 0.5 + 0.5, 0.0, 1.0);
    var col = mix(srgb3(frame.sky_horizon.rgb), srgb3(frame.sky_top.rgb), pow(up, 0.8));

    // Sun disc + halo along the light direction.
    let sun = normalize(frame.sun_dir.xyz);
    let cos_a = dot(d, sun);
    let disc = smoothstep(0.9993, 0.9997, cos_a);
    let halo = pow(clamp(cos_a, 0.0, 1.0), 350.0) * 0.35;
    col = col + srgb3(frame.sun_color.rgb) * (disc + halo);

    // Blend a touch of fog at the horizon.
    let fog_amt = pow(1.0 - up, 4.0) * clamp(frame.fog_color.a * 220.0, 0.0, 0.85);
    col = mix(col, srgb3(frame.fog_color.rgb), fog_amt);

    return vec4<f32>(col, 1.0);
}
