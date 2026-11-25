//! Embedded shader validation: wgpu only parses/validates WGSL at runtime on
//! the GPU path, which no CI machine can exercise. Parse + validate both
//! shaders with naga (the same front-end wgpu uses) so mistakes break
//! `cargo test` instead of the client on launch.

#[test]
fn gfx_shaders_parse_and_validate() {
	let sources: [(&str, &str); 2] =
		[("ui.wgsl", include_str!("ui.wgsl")), ("scene.wgsl", include_str!("scene.wgsl"))];
	for (name, src) in sources {
		let module = naga::front::wgsl::parse_str(src)
			.unwrap_or_else(|e| panic!("{name}: WGSL parse error: {e}"));
		let mut validator = naga::valid::Validator::new(
			naga::valid::ValidationFlags::all(),
			naga::valid::Capabilities::all(),
		);
		validator
			.validate(&module)
			.unwrap_or_else(|e| panic!("{name}: WGSL validation error: {e}"));
		// Entry points must exist.
		assert!(!module.entry_points.is_empty(), "{name}: no entry points");
	}
}
