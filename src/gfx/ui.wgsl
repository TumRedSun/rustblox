// UI pipeline: one instanced quad shader for every 2D primitive
// (rects, SDF rounded boxes, borders, circles, rings, atlas glyphs, images).

struct UiUniforms {
    resolution: vec2<f32>,
    srgb: f32,        // 1.0 when the surface format is sRGB (colors converted in-shader)
    _pad: f32,
};

@group(0) @binding(0) var<uniform> u: UiUniforms;
@group(0) @binding(1) var src_tex: texture_2d<f32>;
@group(0) @binding(2) var src_samp: sampler;

// Instance fields (see gfx::UiInstance):
//  pos_size: x, y, w, h           (px; for rotated prims x/y is the CENTER)
//  color:    rgba (0..1, gamma space)
//  params:   radius, angle, border, feather
//  uv:       texture rect (normalized); glyphs/images
//  misc:     x = kind, y = page (atlas pages unused today), z/w spare
struct UiInstance {
    @location(0) pos_size: vec4<f32>,
    @location(1) color: vec4<f32>,
    @location(2) params: vec4<f32>,
    @location(3) uv: vec4<f32>,
    @location(4) @interpolate(flat) misc: vec4<u32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    // Pixel coords relative to the primitive center, pre-rotation.
    @location(0) local: vec2<f32>,
    @location(1) half_size: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) params: vec4<f32>,
    @location(5) @interpolate(flat) misc: vec4<u32>,
};

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    inst: UiInstance,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let c = corners[vi];
    var out: VsOut;
    let kind = inst.misc.x;

    var x0 = inst.pos_size.x;
    var y0 = inst.pos_size.y;
    var w = inst.pos_size.z;
    var h = inst.pos_size.w;
    var angle = inst.params.y;

    // Solid rects stay axis-aligned with integer corners (crisp edges).
    if (kind == 0u) {
        angle = 0.0;
    }

    // For rotated prims (x,y) is the center; expand to the box and rotate.
    let hw = w * 0.5;
    let hh = h * 0.5;
    let local_raw = (c - vec2<f32>(0.5)) * vec2<f32>(w, h);
    let s = sin(angle);
    let co = cos(angle);
    let rotated = vec2<f32>(
        local_raw.x * co - local_raw.y * s,
        local_raw.x * s + local_raw.y * co,
    );
    // Center position: for rotated prims it is (x0,y0) directly; for the
    // rest derive from top-left.
    var center = vec2<f32>(x0, y0);
    if (kind != 2u && kind != 3u) {
        center = vec2<f32>(x0 + hw, y0 + hh);
    }
    let world = center + rotated;

    let ndc = vec2<f32>(
        world.x / u.resolution.x * 2.0 - 1.0,
        1.0 - world.y / u.resolution.y * 2.0,
    );
    out.pos = vec4<f32>(ndc, 0.0, 1.0);

    // Local px coords relative to center (used by the SDF fragment code).
    // Inverse-rotate the world offset back into prim space.
    let inv_rot = vec2<f32>(
        (world.x - center.x) * co + (world.y - center.y) * s,
        -(world.x - center.x) * s + (world.y - center.y) * co,
    );
    out.local = inv_rot;
    out.half_size = vec2<f32>(hw, hh);

    // UVs: glyphs and images sample the source texture.
    var uv = vec2<f32>(0.0, 0.0);
    if (kind == 6u || kind == 7u) {
        uv = vec2<f32>(
            inst.uv.x + c.x * inst.uv.z,
            inst.uv.y + c.y * inst.uv.w,
        );
    }
    out.uv = uv;
    out.color = inst.color;
    out.params = inst.params;
    out.misc = inst.misc;
    return out;
}

fn srgb_to_linear(c: f32) -> f32 {
    return select(c / 12.92, pow((c + 0.055) / 1.055, 2.4), c > 0.04045);
}

fn to_linear(color: vec4<f32>) -> vec4<f32> {
    if (u.srgb > 0.5) {
        return vec4<f32>(srgb_to_linear(color.r), srgb_to_linear(color.g), srgb_to_linear(color.b), color.a);
    }
    return color;
}

// Signed distance of a rounded box at `p` with half-size `b` and radius `r`.
fn sd_round_box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - (b - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let kind = in.misc.x;
    var alpha = 1.0;
    var color = in.color;

    switch kind {
        // 0: solid rect, 1: rounded rect fill
        case 0u, 1u: {
            if (kind == 1u) {
                let d = sd_round_box(in.local, in.half_size, max(in.params.x, 0.0));
                alpha = clamp(0.5 - d, 0.0, 1.0);
            }
        }
        // 2: rotated rounded rect (same SDF, rotation handled in VS)
        case 2u: {
            let d = sd_round_box(in.local, in.half_size, max(in.params.x, 0.0));
            alpha = clamp(0.5 - d, 0.0, 1.0);
        }
        // 3: rounded rect border (params.z = border width)
        case 3u: {
            let d = abs(sd_round_box(in.local, in.half_size, max(in.params.x, 0.0)));
            let bw = max(in.params.z, 0.5);
            alpha = clamp(0.5 + bw * 0.5 - d, 0.0, 1.0);
        }
        // 4: circle
        case 4u: {
            let d = length(in.local) - in.params.x;
            alpha = clamp(0.5 - d, 0.0, 1.0);
        }
        // 5: ring (params.x = outer r, params.z = inner r)
        case 5u: {
            let r = length(in.local);
            let outer = in.params.x;
            let inner = in.params.z;
            let d_out = r - outer;
            let d_in = inner - r;
            alpha = clamp(0.5 - max(d_out, d_in), 0.0, 1.0);
        }
        // 6: glyph from the atlas (alpha in .r)
        case 6u: {
            let a = textureSample(src_tex, src_samp, in.uv).r;
            alpha = a * in.color.a;
            color = vec4<f32>(in.color.rgb, 1.0);
        }
        // 7: image (rounded corners via params.x radius)
        case 7u: {
            var tex = textureSample(src_tex, src_samp, in.uv);
            if (in.params.x > 0.0) {
                let d = sd_round_box(in.local, in.half_size, in.params.x);
                alpha = clamp(0.5 - d, 0.0, 1.0);
            }
            color = vec4<f32>(tex.rgb, tex.a);
        }
        default: {}
    }

    if (alpha <= 0.001) {
        discard;
    }
    let rgb = to_linear(vec4<f32>(color.rgb, 1.0)).rgb;
    return vec4<f32>(rgb * alpha, alpha);
}
