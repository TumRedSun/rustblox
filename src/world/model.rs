//! Instance tree: properties, node pool and the replication-facing CRUD.

use super::scene_view::{apply_lighting, part_from_instance};
use crate::gfx::scene::{Environment, SceneView};
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
	CFrame {
		pos: [f32; 3],
		rot: [f32; 9],
	},
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
	/// Readable strings harvested from the replication stream (class and
	/// property names from the schema dictionary, part names, usernames).
	pub strings: std::collections::BTreeSet<String>,
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
		let mut env = Environment::default();
		for id in self.walk(start) {
			let Some(inst) = self.get(id) else { continue };
			match inst.class.as_str() {
				"Part" | "WedgePart" | "CornerWedgePart" | "SpawnLocation" | "Seat"
				| "VehicleSeat" | "TrussPart" | "NegatePart" | "UnionOperation" | "MeshPart"
				| "FlagStand" | "RotateP" | "RotateV" | "Glue" => {
					if let Some(part) = part_from_instance(inst) {
						out.parts.push(part);
						count += 1;
					}
				}
				"Lighting" => {
					apply_lighting(inst, &mut env);
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
