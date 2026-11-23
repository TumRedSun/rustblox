//! Per-frame render: atlas sync, instance uploads, then the shadow, main and
//! UI passes onto the surface.

use super::ui::UiUniforms;
use super::Gfx;
use crate::draw::{Clip, Prim};
use crate::gfx::scene::SceneView;

impl Gfx {
	/// Render one frame. Returns `Ok(false)` when the surface is unavailable
	/// (outdated/lost) and the frame was skipped.
	pub fn render(
		&mut self,
		prims: &[(Prim, Clip)],
		atlas: &mut crate::font::AtlasState,
		scene: Option<&SceneView>,
	) -> Result<bool, ()> {
		self.frame_no += 1;
		self.sync_atlas(atlas);
		// The pending masks were just uploaded; drop them so the next frame
		// only uploads glyphs rasterized since. Leaving the list intact made
		// every frame re-upload the ENTIRE accumulated glyph set — per-frame
		// upload cost (and driver staging pressure) grew with the session,
		// which is the progressive-freeze / memory-bloat bug.
		atlas.pending.clear();

		// UI uniforms.
		self.queue.write_buffer(
			&self.ui.uniform,
			0,
			bytemuck::bytes_of(&UiUniforms {
				resolution: [self.config.width as f32, self.config.height as f32],
				srgb: 0.0,
				_pad: 0.0,
			}),
		);
		self.pack_ui(prims);
		if !self.scratch_ui.is_empty() {
			self.queue.write_buffer(&self.ui.instances, 0, bytemuck::cast_slice(&self.scratch_ui));
		}

		let has_scene = scene.is_some();
		if let Some(sc) = scene {
			let fu = self.build_frame_uniforms(&sc.camera, &sc.env);
			self.queue.write_buffer(&self.scene.frame_uniform, 0, bytemuck::bytes_of(&fu));
			self.pack_scene(sc);
			if !self.scratch_scene.is_empty() {
				self.queue.write_buffer(
					&self.scene.instances,
					0,
					bytemuck::cast_slice(&self.scratch_scene),
				);
			}
		}

		let frame = match self.surface.get_current_texture() {
			wgpu::CurrentSurfaceTexture::Success(t) => t,
			wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
			// Skip the frame; the surface will be reconfigured on resize.
			_ => return Ok(false),
		};
		let view = frame.texture.create_view(&Default::default());
		let mut encoder = self
			.device
			.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

		// --- Shadow pass ---------------------------------------------------
		if has_scene && !self.scratch_scene.is_empty() {
			let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
				label: Some("shadow"),
				color_attachments: &[],
				depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
					view: &self.scene.shadow_tex,
					depth_ops: Some(wgpu::Operations {
						load: wgpu::LoadOp::Clear(1.0),
						store: wgpu::StoreOp::Store,
					}),
					stencil_ops: None,
				}),
				timestamp_writes: None,
				occlusion_query_set: None,
				multiview_mask: None,
			});
			pass.set_pipeline(&self.scene.shadow_pipeline);
			pass.set_bind_group(0, &self.scene.group0, &[]);
			pass.set_vertex_buffer(0, self.scene.instances.slice(..));
			let n = self.scratch_scene.len() as u32;
			pass.draw(0..36, 0..n);
		}

		// --- Main pass -----------------------------------------------------
		let clear: [f64; 4] = match scene {
			Some(_) => [0.35, 0.55, 0.85, 1.0],
			None => [0.06, 0.07, 0.09, 1.0],
		};
		let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
			label: Some("main"),
			color_attachments: &[Some(wgpu::RenderPassColorAttachment {
				view: &view,
				depth_slice: None,
				resolve_target: None,
				ops: wgpu::Operations {
					load: wgpu::LoadOp::Clear(wgpu::Color {
						r: clear[0],
						g: clear[1],
						b: clear[2],
						a: clear[3],
					}),
					store: wgpu::StoreOp::Store,
				},
			})],
			depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
				view: &self.depth,
				depth_ops: Some(wgpu::Operations {
					load: wgpu::LoadOp::Clear(1.0),
					store: wgpu::StoreOp::Store,
				}),
				stencil_ops: None,
			}),
			timestamp_writes: None,
			occlusion_query_set: None,
			multiview_mask: None,
		});

		if has_scene {
			// Sky first (no depth interaction).
			pass.set_pipeline(&self.scene.sky_pipeline);
			pass.set_bind_group(0, &self.scene.group0, &[]);
			pass.draw(0..3, 0..1);

			// Lit geometry.
			if !self.scratch_scene.is_empty() {
				pass.set_pipeline(&self.scene.pipeline);
				pass.set_bind_group(0, &self.scene.group0, &[]);
				pass.set_bind_group(1, &self.scene.group1, &[]);
				pass.set_vertex_buffer(0, self.scene.instances.slice(..));
				let n = self.scratch_scene.len() as u32;
				pass.draw(0..36, 0..n);
			}
		}

		// --- UI on top -----------------------------------------------------
		pass.set_pipeline(&self.ui.pipeline);
		pass.set_vertex_buffer(0, self.ui.instances.slice(..));

		let screen_w = self.config.width;
		let screen_h = self.config.height;
		let mut batch_start: usize = 0;
		let mut cur_clip: Option<Clip> = None;
		let mut cur_img: Option<u64> = None;
		let n = prims.len();
		for i in 0..=n {
			let next = if i < n { Some(&prims[i]) } else { None };
			let (img, clip) = match next {
				Some((Prim::Image { hash, .. }, c)) => (Some(*hash), *c),
				Some((_, c)) => (None, *c),
				None => (None, Clip::full(0, 0)), // sentinel: force a flush
			};
			let same_batch = match (cur_clip, cur_img, next.is_some()) {
				(None, _, _) => false,
				(_, _, false) => false,
				(Some(c), cur, true) => {
					c == clip
						&& match (cur, img) {
							(None, None) => true,
							(Some(a), Some(b)) => a == b,
							_ => false,
						}
				}
			};
			if same_batch {
				continue;
			}
			// Flush the finished batch.
			if let (Some(c), start_ok) = (cur_clip, batch_start < i) {
				if start_ok {
					ui_flush(&mut pass, screen_w, screen_h, batch_start as u32, i as u32, c);
				}
			}
			// Start the next batch (bind the right texture group).
			match img {
				Some(h) => {
					match self.images.get(&h) {
						Some(group) => pass.set_bind_group(0, &group.group, &[]),
						// The image failed to upload (bad data, size guard)
						// — draw the quad against the atlas instead of
						// leaving slot 0 unbound: a draw with a missing
						// required bind group is recorded as a lazy
						// validation error that surfaces at finish().
						None => pass.set_bind_group(0, &self.ui.bind_atlas, &[]),
					}
				}
				None => {
					if next.is_some() {
						pass.set_bind_group(0, &self.ui.bind_atlas, &[]);
					}
				}
			}
			cur_clip = Some(clip);
			cur_img = if next.is_some() { img } else { None };
			batch_start = i;
		}
		drop(pass);

		self.queue.submit([encoder.finish()]);
		frame.present();
		Ok(true)
	}
}

fn clip_to_scissor(c: Clip, w: u32, h: u32) -> (u32, u32, u32, u32) {
	let x0 = c.x0.clamp(0, w as i32) as u32;
	let y0 = c.y0.clamp(0, h as i32) as u32;
	let x1 = c.x1.clamp(0, w as i32) as u32;
	let y1 = c.y1.clamp(0, h as i32) as u32;
	(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
}

/// Flush one UI batch: set the scissor to the batch clip and draw the
/// instance range.
fn ui_flush<'p>(
	pass: &mut wgpu::RenderPass<'p>,
	screen_w: u32,
	screen_h: u32,
	start: u32,
	end: u32,
	clip: Clip,
) {
	let (x, y, w, h) = clip_to_scissor(clip, screen_w, screen_h);
	// An empty or fully off-screen clip has nothing to draw; a clamped
	// 1x1 scissor could stick out of the target and fail validation.
	if w == 0 || h == 0 {
		return;
	}
	pass.set_scissor_rect(x, y, w, h);
	pass.draw(0..6, start..end);
}
