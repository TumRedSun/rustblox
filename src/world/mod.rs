//! Runtime DataModel: the instance tree that the replication stream fills
//! and the 3D scene builder reads.
//!
//! Mirrors the Roblox object model in miniature: instances have a class,
//! a name, properties and parent/children links. Only the classes and
//! properties the renderer cares about are stored; everything else the
//! replication parser skips.

use crate::gfx::math::{quat_to_mat3, Vec3};
use crate::gfx::scene::{Environment, ScenePart, SceneView};
use std::collections::HashMap;

pub type InstId = u32;
pub type RefId = i64;

/// A property value in the DataModel.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Str(String),
    Bool(bool),
    I32(i32),
    F32(f32),
    F64(f64),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    /// Position + column-major 3x3 rotation.
    CFrame { pos: [f32; 3], rot: [f32; 9] },
    Color3([f32; 3]),
    Color3u8([u8; 3]),
    Ref(RefId),
    Token(u32),
    Raw(Vec<u8>),
}

impl Prop {
    pub fn as_v3(&self) -> Option<[f32; 3]> {
        match self {
            Prop::Vec3(v) => Some(*v),
            _ => None,
        }
    }
    pub fn as_f32(&self) -> Option<f32> {
        match self {
            Prop::F32(v) => Some(*v),
            Prop::F64(v) => Some(*v as f32),
            Prop::I32(v) => Some(*v as f32),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Prop::Str(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Instance {
    pub id: InstId,
    pub r: RefId,
    pub class: String,
    pub name: String,
    pub parent: Option<InstId>,
    pub children: Vec<InstId>,
    pub props: HashMap<String, Prop>,
}

impl Instance {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.get(name)
    }
}

/// Id-branded instance pool. Ref ids come from the replication stream
/// (-1 means "no reference").
#[derive(Default)]
pub struct DataModel {
    nodes: Vec<Option<Instance>>,
    refs: HashMap<RefId, InstId>,
    pub root: Option<InstId>,
    /// Bumped whenever anything changes (cheap dirty tracking).
    pub version: u64,
    /// Camera CFrame last reported by the server.
    pub camera_cframe: Option<([f32; 3], [f32; 9])>,
}

impl DataModel {
    pub fn new() -> DataModel {
        DataModel::default()
    }

    pub fn get(&self, id: InstId) -> Option<&Instance> {
        self.nodes.get(id as usize).and_then(|n| n.as_ref())
    }

    pub fn get_by_ref(&self, r: RefId) -> Option<&Instance> {
        self.refs.get(&r).and_then(|id| self.get(*id))
    }

    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|n| n.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Create (or replace) an instance for a replication ref.
    pub fn create(&mut self, r: RefId, class: &str, name: &str, parent_ref: RefId) -> InstId {
        self.version += 1;
        // Reuse the existing slot if the ref already maps.
        if let Some(existing) = self.refs.get(&r).copied() {
            let parent = self.refs.get(&parent_ref).copied();
            if let Some(node) = self.nodes.get_mut(existing as usize) {
                if let Some(n) = node {
                    n.class = class.to_string();
                    n.name = name.to_string();
                    n.parent = parent;
                    n.children.clear();
                }
            }
            return existing;
        }
        let id = {
            // Find a free slot.
            match self.nodes.iter().position(|n| n.is_none()) {
                Some(pos) => pos as InstId,
                None => self.nodes.len() as InstId,
            }
        };
        if id as usize == self.nodes.len() {
            self.nodes.push(None);
        }
        let parent = self.refs.get(&parent_ref).copied();
        self.nodes[id as usize] = Some(Instance {
            id,
            r,
            class: class.to_string(),
            name: name.to_string(),
            parent,
            children: Vec::new(),
            props: HashMap::new(),
        });
        self.refs.insert(r, id);
        if let Some(p) = parent {
            if let Some(pn) = self.nodes.get_mut(p as usize) {
                if let Some(pn) = pn {
                    pn.children.push(id);
                }
            }
        }
        if class == "DataModel" {
            self.root = Some(id);
        }
        id
    }

    pub fn remove(&mut self, r: RefId) -> bool {
        let Some(id) = self.refs.get(&r).copied() else {
            return false;
        };
        self.version += 1;
        // Detach from the parent.
        if let Some(node) = self.nodes.get(id as usize).and_then(|n| n.as_ref()) {
            let parent = node.parent;
            if let Some(p) = parent {
                if let Some(pn) = self.nodes.get_mut(p as usize) {
                    if let Some(pn) = pn {
                        pn.children.retain(|c| *c != id);
                    }
                }
            }
        }
        // Remove the subtree.
        let mut stack = vec![id];
        while let Some(i) = stack.pop() {
            if let Some(Some(n)) = self.nodes.get(i as usize).map(|n| n.clone()) {
                stack.extend(n.children.iter().copied());
                for c in n.children {
                    if let Some(cn) = self.nodes.get_mut(c as usize) {
                        *cn = None;
                    }
                }
            }
            if let Some(n) = self.nodes.get_mut(i as usize) {
                *n = None;
            }
        }
        self.refs.remove(&r);
        true
    }

    pub fn set_prop(&mut self, r: RefId, name: &str, value: Prop) {
        let Some(id) = self.refs.get(&r).copied() else {
            return;
        };
        if let Some(Some(node)) = self.nodes.get_mut(id as usize) {
            node.props.insert(name.to_string(), value);
            self.version += 1;
        }
    }

    /// Walk the tree depth-first from `start`.
    pub fn walk(&self, start: InstId) -> Vec<InstId> {
        let mut out = Vec::new();
        let mut stack = vec![start];
        while let Some(i) = stack.pop() {
            if let Some(Some(n)) = self.nodes.get(i as usize).map(|n| n.as_ref()) {
                out.push(i);
                for c in n.children.iter().rev() {
                    stack.push(*c);
                }
            }
        }
        out
    }

    pub fn find_root(&self, class: &str) -> Option<InstId> {
        self.nodes.iter().flatten().find(|n| n.class == class).map(|n| n.id)
    }

    /// Build the renderable scene from the DataModel.
    pub fn build_scene(&self, out: &mut SceneView) -> usize {
        let start = match self.find_root("DataModel").or(self.root) {
            Some(r) => r,
            None => match self.nodes.iter().position(|n| n.is_some()) {
                Some(p) => p as InstId,
                None => return 0,
            },
        };
        let mut count = 0usize;
        let env_dirty = true;
        let mut env = Environment::default();
        for id in self.walk(start) {
            let Some(inst) = self.get(id) else { continue };
            match inst.class.as_str() {
                "Part" | "WedgePart" | "CornerWedgePart" | "SpawnLocation" | "Seat"
                | "VehicleSeat" | "TrussPart" | "NegatePart" | "UnionOperation"
                | "MeshPart" | "FlagStand" | "RotateP" | "RotateV" | "Glue" => {
                    if let Some(part) = part_from_instance(inst) {
                        out.parts.push(part);
                        count += 1;
                    }
                }
                "Lighting" => {
                    apply_lighting(inst, &mut env, env_dirty);
                }
                _ => {
                    // Unknown classes are invisible but kept in the tree.
                }
            }
        }
        out.env = env;
        count
    }
}

/// BrickColor palette id → RGB (the 64 classic BrickColors, condensed to the
/// most common entries; unknown ids fall back to medium grey).
pub fn brick_color(id: u64) -> [f32; 3] {
    let (r, g, b) = match id {
        1 => (0x7f, 0x7f, 0x7f),      // White
        2 => (0xc4, 0xc4, 0xc4),      // Grey
        3 => (0x99, 0x99, 0x99),      // Light yellow
        5 => (0xa1, 0xc4, 0x8c),      // Brick yellow
        6 => (0x6b, 0x74, 0x7e),      // Light blue
        7 => (0x01, 0x9e, 0xd6),      // Bright blue
        8 => (0xf5, 0xcd, 0x30),      // Bright yellow
        9 => (0xd7, 0xc0, 0x9a),      // Earth orange
        10 => (0x6b, 0x32, 0x7c),     // Bright violet
        11 => (0x75, 0x1f, 0x1f),     // Bright red
        12 => (0x4b, 0x97, 0x4b),     // Bright green
        13 => (0x5a, 0x5a, 0x5a),     // Medium grey
        14 => (0x1b, 0x2a, 0x35),     // Dark navy
        15 => (0xa3, 0xa2, 0xa5),     // Light grey
        16 => (0x27, 0xa1, 0x62),     // Cyan
        17 => (0xab, 0xd2, 0xc2),     // Sand
        18 => (0x00, 0x8f, 0x9c),     // Teal
        19 => (0xa1, 0x5f, 0x1d),     // Dark orange
        20 => (0x6a, 0x4f, 0x30),     // Brown
        21 => (0x50, 0x2a, 0x1d),     // Dark brown
        22 => (0x1f, 0x3d, 0x2f),     // Deep green
        23 => (0x11, 0x3c, 0x53),     // Midnight blue
        24 => (0x7f, 0x10, 0x10),     // Maroon
        25 => (0x3c, 0x3c, 0x3c),     // Dark grey
        26 => (0x00, 0x00, 0x00),     // Really black
        27 => (0xf2, 0xf3, 0xf3),     // Institutional white
        28 => (0x20, 0x42, 0x7e),     // Navy blue
        29 => (0x5b, 0x78, 0xc8),     // Steel blue
        30 => (0x7c, 0x9d, 0xe8),     // Cornflower blue
        31 => (0x8c, 0x88, 0xc8),     // Royal purple
        32 => (0xa7, 0x8f, 0xd1),     // Lilac
        33 => (0xf8, 0xba, 0xa0),     // Salmon
        34 => (0xfa, 0x86, 0x7a),     // Coral
        35 => (0xf6, 0x4b, 0x3e),     // Orange red
        36 => (0xf5, 0xd4, 0x44),     // Bright orange
        37 => (0xe2, 0x9c, 0x1e),     // Gold
        38 => (0x9c, 0x8b, 0x2c),     // Olive
        39 => (0xa4, 0xbd, 0x47),     // Lime green
        40 => (0x74, 0xb6, 0x3c),     // Bright green
        41 => (0x53, 0x9b, 0x34),     // Sea green
        42 => (0x1c, 0x5b, 0x2f),     // Forest green
        43 => (0x79, 0xc2, 0x7c),     // Mint
        44 => (0x0b, 0xb8, 0xe2),     // Bright cyan
        45 => (0x80, 0xbb, 0xdb),     // Baby blue
        46 => (0x9f, 0xd5, 0xec),     // Pastel blue
        47 => (0xc7, 0xd4, 0xe5),     // Fog blue
        48 => (0xd4, 0xd7, 0xd9),     // Silver
        49 => (0x6d, 0x6e, 0x71),     // Slate
        50 => (0x21, 0x29, 0x30),     // Charcoal
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

fn part_from_instance(inst: &Instance) -> Option<ScenePart> {
    // Position: CFrame preferred, Position fallback (axis-aligned).
    let (pos, rot) = match inst.prop("CFrame") {
        Some(Prop::CFrame { pos, rot }) => (*pos, *rot),
        _ => {
            let p = inst.prop("Position").and_then(|p| p.as_v3()).unwrap_or([0.0; 3]);
            (p, [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
        }
    };
    let size = inst.prop("Size").and_then(|p| p.as_v3()).unwrap_or([4.0, 1.2, 2.0]);

    let color: [f32; 3] = match (inst.prop("Color3uint8"), inst.prop("Color"), inst.prop("BrickColor")) {
        (Some(Prop::Color3u8(c)), _, _) => [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0],
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
    let transparency = inst.prop("Transparency").and_then(|p| p.as_f32()).unwrap_or(0.0).clamp(0.0, 1.0);
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

fn apply_lighting(inst: &Instance, env: &mut Environment, _dirty: bool) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let p = dm.create(-12, "Part", "Brick", -11);
        dm.set_prop(-12, "CFrame", Prop::CFrame { pos: [0.0, 5.0, 0.0], rot: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] });
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
}
