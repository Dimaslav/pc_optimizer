//! Функции кодирования рендер-проходов.

use crate::render::csm::CASCADE_COUNT;
use crate::render::debug::DebugView;
use crate::render::line::LineVertex;
use crate::render::mesh::InstanceData;

use super::gpu_types::*;
use super::size_dep::BLOOM_MIP_COUNT;
use super::Renderer;

#[inline]
fn fullscreen_triangle(pass: &mut wgpu::RenderPass<'_>) {
    pass.draw(0..3, 0..1);
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn draw_all_instances<'a>(
    pass: &mut wgpu::RenderPass<'a>,
    r: &'a Renderer,
    draws: &[MeshDraw],
    bind_material_at: Option<u32>,
    include_blend: bool,
    pipeline_single: &'a wgpu::RenderPipeline,
    pipeline_double: &'a wgpu::RenderPipeline,
) {
    let instance_stride = std::mem::size_of::<InstanceData>() as u64;
    let mut offset_bytes: u64 = 0;

    for d in draws {
        if d.instances.is_empty() {
            continue;
        }

        if d.blend != include_blend {
            offset_bytes += d.instances.len() as u64 * instance_stride;
            continue;
        }

        let Some(mesh) = r.meshes.get(&d.mesh) else {
            offset_bytes += d.instances.len() as u64 * instance_stride;
            continue;
        };

        let pipeline = if d.double_sided {
            pipeline_double
        } else {
            pipeline_single
        };
        pass.set_pipeline(pipeline);

        if let Some(slot) = bind_material_at {
            let mat_gpu = d
                .texture
                .as_deref()
                .and_then(|n| r.material_bind_groups.get(n))
                .unwrap_or(&r.default_material_bind_group);
            pass.set_bind_group(slot, &mat_gpu.bind_group, &[]);
        }

        pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, r.instance_buffer.slice(offset_bytes..));
        pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.index_count, 0, 0..d.instances.len() as u32);

        offset_bytes += d.instances.len() as u64 * instance_stride;
    }
}

#[inline]
fn clear(color: wgpu::Color) -> wgpu::Operations<wgpu::Color> {
    wgpu::Operations {
        load: wgpu::LoadOp::Clear(color),
        store: wgpu::StoreOp::Store,
    }
}

#[inline]
fn load() -> wgpu::Operations<wgpu::Color> {
    wgpu::Operations {
        load: wgpu::LoadOp::Load,
        store: wgpu::StoreOp::Store,
    }
}

#[inline]
fn depth_clear(value: f32) -> wgpu::Operations<f32> {
    wgpu::Operations {
        load: wgpu::LoadOp::Clear(value),
        store: wgpu::StoreOp::Store,
    }
}

#[inline]
fn depth_load() -> wgpu::Operations<f32> {
    wgpu::Operations {
        load: wgpu::LoadOp::Load,
        store: wgpu::StoreOp::Store,
    }
}

// ============================================================
// Shadow
// ============================================================

pub(super) fn encode_shadow_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    draws: &[MeshDraw],
    view: &wgpu::TextureView,
    slot_offset: u32,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("shadow_pass"),
        color_attachments: &[],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view,
            depth_ops: Some(depth_clear(1.0)),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_bind_group(0, &r.shadow_pass_bind_group, &[slot_offset]);
    draw_all_instances(
        &mut pass,
        r,
        draws,
        Some(1),
        false,
        &r.shadow_pipeline,
        &r.shadow_pipeline_double_sided,
    );
}

pub(super) fn encode_csm_all(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    draws: &[MeshDraw],
) {
    for cascade in 0..CASCADE_COUNT {
        let slot_offset = (cascade as u64 * r.shadow_pass_stride) as u32;
        encode_shadow_pass(r, encoder, draws, &r.csm_cascade_views[cascade], slot_offset);
    }
}

/// ИЗМЕНЕНО (#7): рендерит cube shadow maps для **всех** активных
/// point-light теней, а не только для первой. `cube_count` ограничен
/// `MAX_SHADOW_CUBES` сверху (значение приходит из `Renderer::render`).
///
/// Слоты нумеруются как `3 + cube * 6 + face`, что совпадает с
/// нумерацией слоёв в `cube_shadow_texture` — слой `cube * 6 + face`.
pub(super) fn encode_cube_shadow_all(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    draws: &[MeshDraw],
    cube_count: u32,
) {
    let cube_count = (cube_count as usize).min(MAX_SHADOW_CUBES);
    for cube in 0..cube_count {
        for face in 0..6 {
            let slot = 3 + (cube * 6 + face) as u64;
            let slot_offset = (slot * r.shadow_pass_stride) as u32;
            encode_shadow_pass(
                r,
                encoder,
                draws,
                &r.cube_shadow_face_views[cube][face],
                slot_offset,
            );
        }
    }
}

// ============================================================
// G-buffer
// ============================================================

pub(super) fn encode_gbuffer_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    draws: &[MeshDraw],
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gbuffer_pass"),
        color_attachments: &[
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.gbuffer_albedo_view,
                resolve_target: None,
                ops: clear(wgpu::Color::BLACK),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.gbuffer_normal_view,
                resolve_target: None,
                ops: clear(wgpu::Color::BLACK),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.gbuffer_emissive_view,
                resolve_target: None,
                ops: clear(wgpu::Color::BLACK),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.motion_view,
                resolve_target: None,
                ops: clear(wgpu::Color::TRANSPARENT),
            }),
        ],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &r.sd.gbuffer_depth_view,
            depth_ops: Some(depth_clear(1.0)),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_bind_group(0, &r.camera_bind_group, &[]);
    draw_all_instances(
        &mut pass,
        r,
        draws,
        Some(1),
        false,
        &r.gbuffer_pipeline,
        &r.gbuffer_pipeline_double_sided,
    );
}

// ============================================================
// Decals
// ============================================================

pub(super) fn encode_decal_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    decals: &[crate::render::decal::DecalDraw],
) {
    if decals.is_empty() {
        return;
    }
    if !decals.iter().any(|d| !d.instances.is_empty()) {
        return;
    }

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("decal_pass"),
        color_attachments: &[
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.gbuffer_albedo_view,
                resolve_target: None,
                ops: load(),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.gbuffer_normal_view,
                resolve_target: None,
                ops: load(),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.gbuffer_emissive_view,
                resolve_target: None,
                ops: load(),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.motion_view,
                resolve_target: None,
                ops: load(),
            }),
        ],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_bind_group(0, &r.camera_bind_group, &[]);
    pass.set_bind_group(2, &r.decal_depth_bind_group, &[]);

    let Some(cube) = r.meshes.get("cube") else { return; };
    pass.set_vertex_buffer(0, cube.vertex_buffer.slice(..));
    pass.set_index_buffer(cube.index_buffer.slice(..), wgpu::IndexFormat::Uint32);

    pass.set_pipeline(&r.decal_pipeline);

    let stride = std::mem::size_of::<crate::render::decal::DecalInstance>() as u64;
    let mut offset: u64 = 0;

    for d in decals {
        if d.instances.is_empty() {
            offset += d.instances.len() as u64 * stride;
            continue;
        }
        let Some(bg) = r.decal_texture_bind_groups.get(&d.texture) else {
            offset += d.instances.len() as u64 * stride;
            continue;
        };
        pass.set_bind_group(1, bg, &[]);
        pass.set_vertex_buffer(1, r.decal_instance_buffer.slice(offset..));
        pass.draw_indexed(0..cube.index_count, 0, 0..d.instances.len() as u32);
        offset += d.instances.len() as u64 * stride;
    }
}

// ============================================================
// SSAO
// ============================================================

pub(super) fn encode_ssao_pass(r: &Renderer, encoder: &mut wgpu::CommandEncoder) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("ssao_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.ssao_view,
            resolve_target: None,
            ops: clear(wgpu::Color::WHITE),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.ssao_pipeline);
    pass.set_bind_group(0, &r.sd.ssao_bind_group, &[]);
    fullscreen_triangle(&mut pass);
}

pub(super) fn encode_ssao_blur_pass(r: &Renderer, encoder: &mut wgpu::CommandEncoder) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("ssao_blur_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.ssao_blur_view,
            resolve_target: None,
            ops: clear(wgpu::Color::WHITE),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.ssao_blur_pipeline);
    pass.set_bind_group(0, &r.sd.ssao_blur_bind_group, &[]);
    fullscreen_triangle(&mut pass);
}

// ============================================================
// Lighting
// ============================================================

pub(super) fn encode_lighting_pass(r: &Renderer, encoder: &mut wgpu::CommandEncoder) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("lighting_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.hdr_view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_pipeline(&r.lighting_pipeline);
    pass.set_bind_group(0, &r.sd.lighting_bind_group, &[]);
    pass.set_bind_group(1, &r.lights_bind_group, &[]);
    pass.set_bind_group(2, &r.sd.shadow2_bind_group, &[]);
    fullscreen_triangle(&mut pass);
}

// ============================================================
// Skybox
// ============================================================

pub(super) fn encode_skybox_pass(r: &Renderer, encoder: &mut wgpu::CommandEncoder) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("skybox_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.hdr_view,
            resolve_target: None,
            ops: load(),
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &r.sd.gbuffer_depth_view,
            depth_ops: Some(depth_load()),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_pipeline(&r.skybox_pipeline);
    pass.set_bind_group(0, &r.skybox_bind_group, &[]);
    fullscreen_triangle(&mut pass);
}

// ============================================================
// Forward (lines)
// ============================================================

pub(super) fn encode_forward_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    line_vertices: &[LineVertex],
) {
    if line_vertices.is_empty() && r.line_buffer.vertex_count == 0 {
        return;
    }
    if r.line_buffer.vertex_count == 0 {
        return;
    }

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("forward_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.hdr_view,
            resolve_target: None,
            ops: load(),
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &r.sd.gbuffer_depth_view,
            depth_ops: Some(depth_load()),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_pipeline(&r.line_pipeline);
    pass.set_bind_group(0, &r.camera_bind_group, &[]);
    pass.set_vertex_buffer(0, r.line_buffer.buffer.slice(..));
    pass.draw(0..r.line_buffer.vertex_count, 0..1);
}

// ============================================================
// Particles
// ============================================================

pub(super) fn encode_particles_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    particle_count: u32,
) {
    if particle_count == 0 {
        return;
    }

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("particles_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.hdr_view,
            resolve_target: None,
            ops: load(),
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &r.sd.gbuffer_depth_view,
            depth_ops: Some(depth_load()),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_pipeline(&r.particles_pipeline);
    pass.set_bind_group(0, &r.camera_bind_group, &[]);
    pass.set_vertex_buffer(0, r.particles_instance_buffer.slice(..));
    pass.draw(0..6, 0..particle_count);
}

// ============================================================
// Transparent
// ============================================================

pub(super) fn encode_transparent_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    draws: &[MeshDraw],
) {
    if !draws.iter().any(|d| d.blend && !d.instances.is_empty()) {
        return;
    }

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("transparent_pass"),
        color_attachments: &[
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.hdr_view,
                resolve_target: None,
                ops: load(),
            }),
            Some(wgpu::RenderPassColorAttachment {
                view: &r.sd.motion_view,
                resolve_target: None,
                ops: load(),
            }),
        ],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &r.sd.gbuffer_depth_view,
            depth_ops: Some(depth_load()),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    pass.set_bind_group(0, &r.camera_bind_group, &[]);
    pass.set_bind_group(1, &r.lights_bind_group, &[]);
    pass.set_bind_group(2, &r.sd.shadow2_bind_group, &[]);
    draw_all_instances(
        &mut pass,
        r,
        draws,
        Some(3),
        true,
        &r.transparent_pipeline,
        &r.transparent_pipeline_double_sided,
    );
}

// ============================================================
// Volumetric fog
// ============================================================

pub(super) fn encode_volumetric_compute(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
) {
    let mut cpass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some("volumetric_compute_pass"),
        timestamp_writes: None,
    });
    cpass.set_pipeline(&r.volumetric_pipeline);
    cpass.set_bind_group(0, &r.sd.volumetric_compute_bg, &[]);
    let gx = (VOLUMETRIC_GRID_W + 7) / 8;
    let gy = (VOLUMETRIC_GRID_H + 7) / 8;
    let gz = (VOLUMETRIC_GRID_D + 3) / 4;
    cpass.dispatch_workgroups(gx, gy, gz);
}

pub(super) fn encode_volumetric_composite(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("volumetric_composite_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.hdr_fog_view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.volumetric_composite_pipeline);
    pass.set_bind_group(0, &r.sd.volumetric_composite_bg, &[]);
    fullscreen_triangle(&mut pass);
}

// ============================================================
// TAA
// ============================================================

pub(super) fn encode_taa_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    write_idx: usize,
) {
    let read_idx = 1 - write_idx;
    let dst_view = &r.sd.taa_resolved_views[write_idx];
    let bg = &r.sd.taa_read_bgs[read_idx];

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("taa_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: dst_view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.taa_pipeline);
    pass.set_bind_group(0, bg, &[]);
    fullscreen_triangle(&mut pass);
}

// ============================================================
// Bloom chain
// ============================================================

pub(super) fn encode_bloom_prefilter(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    write_idx: usize,
    use_taa: bool,
) {
    let mip0 = &r.sd.bloom_chain.mips[0];
    let bg_idx = if use_taa { write_idx } else { 2 };
    let bg = &r.sd.bloom_chain.prefilter_bgs[bg_idx];

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("bloom_prefilter"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &mip0.view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.bloom_prefilter_pipeline);
    pass.set_bind_group(0, bg, &[]);
    fullscreen_triangle(&mut pass);
}

pub(super) fn encode_bloom_downsample(r: &Renderer, encoder: &mut wgpu::CommandEncoder) {
    for i in 1..BLOOM_MIP_COUNT {
        let dst = &r.sd.bloom_chain.mips[i];
        let bg = &r.sd.bloom_chain.downsample_bgs[i - 1];

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bloom_downsample"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
                resolve_target: None,
                ops: clear(wgpu::Color::BLACK),
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&r.bloom_downsample_pipeline);
        pass.set_bind_group(0, bg, &[]);
        fullscreen_triangle(&mut pass);
    }
}

pub(super) fn encode_bloom_upsample(r: &Renderer, encoder: &mut wgpu::CommandEncoder) {
    for idx in 0..(BLOOM_MIP_COUNT - 1) {
        let dst_level = BLOOM_MIP_COUNT - 2 - idx;
        let dst = &r.sd.bloom_chain.mips[dst_level];
        let bg = &r.sd.bloom_chain.upsample_bgs[idx];

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("bloom_upsample"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &dst.view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&r.bloom_upsample_pipeline);
        pass.set_bind_group(0, bg, &[]);
        fullscreen_triangle(&mut pass);
    }
}

pub(super) fn encode_bloom_chain(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    write_idx: usize,
    use_taa: bool,
) {
    encode_bloom_prefilter(r, encoder, write_idx, use_taa);
    encode_bloom_downsample(r, encoder);
    encode_bloom_upsample(r, encoder);
}

// ============================================================
// Post-processing
// ============================================================

pub(super) fn encode_composite_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    write_idx: usize,
    use_taa: bool,
) {
    let bg_idx = if use_taa { write_idx } else { 2 };
    let bg = &r.sd.composite_bgs[bg_idx];

    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("composite_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &r.sd.ldr_view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.tonemap_pipeline);
    pass.set_bind_group(0, bg, &[]);
    fullscreen_triangle(&mut pass);
}

pub(super) fn encode_fxaa_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    swap_view: &wgpu::TextureView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("fxaa_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: swap_view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });
    pass.set_pipeline(&r.fxaa_pipeline);
    pass.set_bind_group(0, &r.sd.fxaa_bind_group, &[]);
    fullscreen_triangle(&mut pass);
}

pub(super) fn encode_post_processing(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    swap_view: &wgpu::TextureView,
    write_idx: usize,
    use_taa: bool,
) {
    encode_bloom_chain(r, encoder, write_idx, use_taa);
    encode_composite_pass(r, encoder, write_idx, use_taa);
    encode_fxaa_pass(r, encoder, swap_view);
}

// ============================================================
// Debug
// ============================================================

pub(super) fn encode_debug_pass(
    r: &Renderer,
    encoder: &mut wgpu::CommandEncoder,
    swap_view: &wgpu::TextureView,
    debug_view: DebugView,
) {
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("debug_pass"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: swap_view,
            resolve_target: None,
            ops: clear(wgpu::Color::BLACK),
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    });

    match debug_view {
        DebugView::Ssao => {
            pass.set_pipeline(&r.debug2d_pipeline);
            pass.set_bind_group(0, &r.sd.debug_bind_ssao, &[]);
        }
        DebugView::GbufferNormal => {
            pass.set_pipeline(&r.debug2d_pipeline);
            pass.set_bind_group(0, &r.sd.debug_bind_gbuffer, &[]);
        }
        DebugView::GbufferDepth => {
            pass.set_pipeline(&r.debug2d_pipeline);
            pass.set_bind_group(0, &r.sd.debug_bind_depth, &[]);
        }
        DebugView::HdrPreBloom => {
            pass.set_pipeline(&r.debug2d_pipeline);
            pass.set_bind_group(0, &r.sd.debug_bind_hdr, &[]);
        }
        DebugView::CsmCascade0 => {
            pass.set_pipeline(&r.debug_depth_pipeline);
            pass.set_bind_group(0, &r.csm_debug_bind_group, &[]);
        }
        DebugView::Final => unreachable!(),
    }
    fullscreen_triangle(&mut pass);
}