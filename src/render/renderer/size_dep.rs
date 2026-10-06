//! Ресурсы, зависящие от размера окна.

use crate::render::ibl::IblResources;

use super::gpu_types::*;

pub const BLOOM_MIP_COUNT: usize = 5;

pub struct BloomMip {
    pub view: wgpu::TextureView,
    pub _tex: wgpu::Texture,
    pub size: (u32, u32),
}

/// ИЗМЕНЕНО (#6): `prefilter_bgs` теперь `[_; 3]`.
///
/// Индекс 0 — prefilter читает `taa_resolved_views[0]` (TAA on, write_idx = 0).
/// Индекс 1 — prefilter читает `taa_resolved_views[1]` (TAA on, write_idx = 1).
/// Индекс 2 — prefilter читает `hdr_fog_view`      (TAA off, bypass).
pub struct BloomChain {
    pub mips: Vec<BloomMip>,
    pub sampler: wgpu::Sampler,
    pub prefilter_bgs: [wgpu::BindGroup; 3],
    pub downsample_bgs: Vec<wgpu::BindGroup>,
    pub upsample_bgs: Vec<wgpu::BindGroup>,
    pub prefilter_uniform: wgpu::Buffer,
    pub downsample_uniforms: Vec<wgpu::Buffer>,
    pub upsample_uniforms: Vec<wgpu::Buffer>,
    /// Размер экрана на момент построения цепочки (src для prefilter).
    pub screen_size: (u32, u32),
}

impl BloomChain {
    /// Обновить threshold / knee / radius во всех uniform'ах цепочки,
    /// сохранив texel-размеры каждого уровня.
    pub fn update_params(
        &self,
        queue: &wgpu::Queue,
        threshold: f32,
        knee: f32,
        radius: f32,
    ) {
        let knee = knee.max(1e-4);
        let radius = radius.max(0.5);

        // --- Prefilter: src = screen, dst = mip0.
        let src = self.screen_size;
        let dst = self.mips[0].size;
        let data = BloomParams {
            texel: [
                1.0 / src.0.max(1) as f32,
                1.0 / src.1.max(1) as f32,
                1.0 / dst.0.max(1) as f32,
                1.0 / dst.1.max(1) as f32,
            ],
            params: [threshold, knee, radius, 0.0],
        };
        queue.write_buffer(&self.prefilter_uniform, 0, bytemuck::bytes_of(&data));

        // --- Downsample: src = mips[i], dst = mips[i + 1].
        for i in 0..self.downsample_uniforms.len() {
            let src = self.mips[i].size;
            let dst = self.mips[i + 1].size;
            let data = BloomParams {
                texel: [
                    1.0 / src.0.max(1) as f32,
                    1.0 / src.1.max(1) as f32,
                    1.0 / dst.0.max(1) as f32,
                    1.0 / dst.1.max(1) as f32,
                ],
                params: [threshold, knee, radius, 0.0],
            };
            queue.write_buffer(
                &self.downsample_uniforms[i],
                0,
                bytemuck::bytes_of(&data),
            );
        }

        // --- Upsample.
        for i in 0..self.upsample_uniforms.len() {
            let src_level = BLOOM_MIP_COUNT - 1 - i;
            let src = self.mips[src_level].size;
            let dst = self.mips[src_level - 1].size;
            let data = BloomParams {
                texel: [
                    1.0 / src.0.max(1) as f32,
                    1.0 / src.1.max(1) as f32,
                    1.0 / dst.0.max(1) as f32,
                    1.0 / dst.1.max(1) as f32,
                ],
                params: [threshold, knee, radius, 0.0],
            };
            queue.write_buffer(
                &self.upsample_uniforms[i],
                0,
                bytemuck::bytes_of(&data),
            );
        }
    }
}

pub struct SizeDependent {
    pub hdr_view: wgpu::TextureView,
    pub motion_view: wgpu::TextureView,

    /// HDR после volumetric fog. TAA читает отсюда.
    pub hdr_fog_view: wgpu::TextureView,
    pub _hdr_fog_tex: wgpu::Texture,

    pub taa_resolved_views: [wgpu::TextureView; 2],
    pub _taa_resolved_tex: [wgpu::Texture; 2],
    pub taa_read_bgs: [wgpu::BindGroup; 2],
    pub taa_uniform: wgpu::Buffer,

    /// Froxel-текстура volumetric fog.
    pub volumetric_fog_view: wgpu::TextureView,
    pub _volumetric_fog_tex: wgpu::Texture,
    /// Bind group для compute-прохода.
    pub volumetric_compute_bg: wgpu::BindGroup,
    /// Bind group для composite-прохода (fog + SSR).
    pub volumetric_composite_bg: wgpu::BindGroup,

    pub bloom_chain: BloomChain,
    pub linear_sampler: wgpu::Sampler,

    /// ИЗМЕНЕНО (#6): `composite_bgs` теперь `[_; 3]`.
    ///
    /// Индекс 0 — composite читает `taa_resolved_views[0]` (TAA on, write_idx = 0).
    /// Индекс 1 — composite читает `taa_resolved_views[1]` (TAA on, write_idx = 1).
    /// Индекс 2 — composite читает `hdr_fog_view`      (TAA off, bypass).
    pub composite_bgs: [wgpu::BindGroup; 3],

    pub ldr_view: wgpu::TextureView,
    pub fxaa_uniform: wgpu::Buffer,
    pub fxaa_bind_group: wgpu::BindGroup,

    pub gbuffer_albedo_view: wgpu::TextureView,
    pub gbuffer_normal_view: wgpu::TextureView,
    pub gbuffer_emissive_view: wgpu::TextureView,
    pub gbuffer_depth_view: wgpu::TextureView,

    pub ssao_view: wgpu::TextureView,
    pub ssao_blur_view: wgpu::TextureView,
    pub ssao_bind_group: wgpu::BindGroup,
    pub ssao_blur_bind_group: wgpu::BindGroup,

    pub shadow2_bind_group: wgpu::BindGroup,
    pub lighting_bind_group: wgpu::BindGroup,

    pub debug_uniform: wgpu::Buffer,
    pub debug_bind_ssao: wgpu::BindGroup,
    pub debug_bind_gbuffer: wgpu::BindGroup,
    pub debug_bind_depth: wgpu::BindGroup,
    pub debug_bind_hdr: wgpu::BindGroup,
}

#[allow(clippy::too_many_arguments)]
pub fn build_size_dependent(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    bloom_layout: &wgpu::BindGroupLayout,
    tonemap_layout: &wgpu::BindGroupLayout,
    ssao_layout: &wgpu::BindGroupLayout,
    shadow2_layout: &wgpu::BindGroupLayout,
    debug_layout: &wgpu::BindGroupLayout,
    lighting_layout: &wgpu::BindGroupLayout,
    taa_layout: &wgpu::BindGroupLayout,
    volumetric_compute_layout: &wgpu::BindGroupLayout,
    volumetric_composite_layout: &wgpu::BindGroupLayout,
    ssao_uniform: &wgpu::Buffer,
    volumetric_uniform: &wgpu::Buffer,
    noise_view: &wgpu::TextureView,
    csm_array_view: &wgpu::TextureView,
    csm_sampler: &wgpu::Sampler,
    // ИЗМЕНЕНО (#7): теперь cube array вместо одного куба.
    // Тип `TextureView` не меняется (view_dimension — свойство view'а,
    // не Rust-типа), но семантически: этот view имеет dimension
    // `CubeArray` и содержит `6 * MAX_SHADOW_CUBES` слоёв.
    cube_shadow_array_view: &wgpu::TextureView,
    cube_shadow_sampler: &wgpu::Sampler,
    camera_buffer: &wgpu::Buffer,
    lights_buffer: &wgpu::Buffer,
    ibl: &IblResources,
    bloom_knee: f32,
    bloom_radius: f32,
    tonemap_uniform: &wgpu::Buffer,
) -> SizeDependent {
    let w = config.width.max(1);
    let h = config.height.max(1);

    let hdr_view = create_color_target(device, "hdr", w, h, HDR_FORMAT, 1, true);
    let motion_view = create_color_target(device, "motion", w, h, MOTION_FORMAT, 1, true);
    let ldr_view = create_color_target(device, "ldr", w, h, LDR_FORMAT, 1, true);
    let linear_sampler = create_linear_sampler(device, "post_linear");

    // ============================================================
    // Volumetric fog (froxel) resources
    // ============================================================
    let volumetric_fog_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("volumetric_fog_3d"),
        size: wgpu::Extent3d {
            width: VOLUMETRIC_GRID_W,
            height: VOLUMETRIC_GRID_H,
            depth_or_array_layers: VOLUMETRIC_GRID_D,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: VOLUMETRIC_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let volumetric_fog_view = volumetric_fog_tex.create_view(&wgpu::TextureViewDescriptor {
        label: Some("volumetric_fog_view"),
        dimension: Some(wgpu::TextureViewDimension::D3),
        ..Default::default()
    });

    let volumetric_compute_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("volumetric_compute_bg"),
        layout: volumetric_compute_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: lights_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: volumetric_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(csm_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(csm_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&volumetric_fog_view),
            },
        ],
    });

    // ============================================================
    // TAA ping-pong resolved
    // ============================================================
    let taa_resolved_tex: [wgpu::Texture; 2] = std::array::from_fn(|i| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(&format!("taa_resolved_{}", i)),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    });
    let taa_resolved_views: [wgpu::TextureView; 2] = std::array::from_fn(|i| {
        taa_resolved_tex[i].create_view(&wgpu::TextureViewDescriptor::default())
    });

    let taa_uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("taa_uniform"),
        size: std::mem::size_of::<TaaParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // ============================================================
    // HDR + volumetric fog composite target.
    // TAA читает отсюда, а не из hdr_view напрямую.
    // ============================================================
    let hdr_fog_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hdr_fog"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: HDR_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let hdr_fog_view = hdr_fog_tex.create_view(&wgpu::TextureViewDescriptor::default());

    let taa_read_bgs: [wgpu::BindGroup; 2] = std::array::from_fn(|i| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("taa_read_bg"),
            layout: taa_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_fog_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&taa_resolved_views[i]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&motion_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: taa_uniform.as_entire_binding(),
                },
            ],
        })
    });

    // ============================================================
    // G-buffer: 3 MRT + depth (+ motion отдельно)
    // ============================================================
    let gbuffer_albedo_view =
        create_color_target(device, "gbuffer_albedo", w, h, GBUFFER_FORMAT, 1, true);
    let gbuffer_normal_view =
        create_color_target(device, "gbuffer_normal", w, h, GBUFFER_FORMAT, 1, true);
    let gbuffer_emissive_view =
        create_color_target(device, "gbuffer_emissive", w, h, GBUFFER_FORMAT, 1, true);
    let gbuffer_depth_view = create_depth_view(device, w, h, 1);

    // ============================================================
    // Volumetric composite bind group (fog + SSR).
    // ============================================================
    let volumetric_composite_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("volumetric_composite_bg"),
        layout: volumetric_composite_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&hdr_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&volumetric_fog_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&linear_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&gbuffer_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: volumetric_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&gbuffer_normal_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&gbuffer_albedo_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&gbuffer_emissive_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: camera_buffer.as_entire_binding(),
            },
        ],
    });

    // ============================================================
    // SSAO
    // ============================================================
    let ssao_view = create_color_target(device, "ssao", w, h, SSAO_FORMAT, 1, true);
    let ssao_blur_view = create_color_target(device, "ssao_blur", w, h, SSAO_FORMAT, 1, true);

    let ssao_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ssao_bind_group"),
        layout: ssao_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&gbuffer_normal_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(noise_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&linear_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: ssao_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&gbuffer_normal_view),
            },
        ],
    });

    let ssao_blur_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ssao_blur_bind_group"),
        layout: ssao_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&ssao_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(noise_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&linear_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: ssao_uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&gbuffer_normal_view),
            },
        ],
    });

    // ИЗМЕНЕНО (#7): shadow2_bind_group binding 2 → cube_shadow_array_view.
    // Layout (`shadow2_layout`) в `mod.rs` приведён к `CubeArray`.
    let shadow2_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("shadow2_bind_group"),
        layout: shadow2_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(csm_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(csm_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(cube_shadow_array_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(cube_shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(&ssao_blur_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(&linear_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&ibl.irradiance_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&ibl.prefiltered_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: wgpu::BindingResource::TextureView(&ibl.brdf_lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::Sampler(&ibl.env_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&ibl.env_cube_view),
            },
        ],
    });

    let lighting_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("lighting_bind_group"),
        layout: lighting_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&gbuffer_albedo_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&gbuffer_normal_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&gbuffer_emissive_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&gbuffer_depth_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&linear_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: camera_buffer.as_entire_binding(),
            },
        ],
    });

    // ============================================================
    // Bloom chain
    // ============================================================
    let bloom_chain = build_bloom_chain(
        device,
        bloom_layout,
        &taa_resolved_views,
        &hdr_fog_view,
        w,
        h,
        bloom_knee,
        bloom_radius,
    );

    // ИЗМЕНЕНО (#6): три варианта composite bind group.
    let composite_bgs: [wgpu::BindGroup; 3] = std::array::from_fn(|i| {
        let src_view: &wgpu::TextureView = if i < 2 {
            &taa_resolved_views[i]
        } else {
            &hdr_fog_view
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("composite_bind_group"),
            layout: tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&bloom_chain.mips[0].view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: tonemap_uniform.as_entire_binding(),
                },
            ],
        })
    });

    let debug_uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("debug_uniform"),
        size: std::mem::size_of::<DebugParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let fxaa_uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fxaa_uniform"),
        size: std::mem::size_of::<FxaaParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let fxaa_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("fxaa_bg"),
        layout: debug_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&ldr_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&linear_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: fxaa_uniform.as_entire_binding(),
            },
        ],
    });

    let make_debug_bg = |name: &str, view: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(name),
            layout: debug_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&linear_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: debug_uniform.as_entire_binding(),
                },
            ],
        })
    };

    let debug_bind_ssao = make_debug_bg("debug_bg_ssao", &ssao_blur_view);
    let debug_bind_gbuffer = make_debug_bg("debug_bg_gbuffer", &gbuffer_normal_view);
    let debug_bind_depth = make_debug_bg("debug_bg_depth", &gbuffer_normal_view);
    let debug_bind_hdr = make_debug_bg("debug_bg_hdr", &hdr_view);

    SizeDependent {
        hdr_view,
        motion_view,
        hdr_fog_view,
        _hdr_fog_tex: hdr_fog_tex,
        taa_resolved_views,
        _taa_resolved_tex: taa_resolved_tex,
        taa_read_bgs,
        taa_uniform,
        volumetric_fog_view,
        _volumetric_fog_tex: volumetric_fog_tex,
        volumetric_compute_bg,
        volumetric_composite_bg,
        bloom_chain,
        linear_sampler,
        composite_bgs,
        ldr_view,
        fxaa_uniform,
        fxaa_bind_group,
        gbuffer_albedo_view,
        gbuffer_normal_view,
        gbuffer_emissive_view,
        gbuffer_depth_view,
        ssao_view,
        ssao_blur_view,
        ssao_bind_group,
        ssao_blur_bind_group,
        shadow2_bind_group,
        lighting_bind_group,
        debug_uniform,
        debug_bind_ssao,
        debug_bind_gbuffer,
        debug_bind_depth,
        debug_bind_hdr,
    }
}

fn build_bloom_chain(
    device: &wgpu::Device,
    bloom_layout: &wgpu::BindGroupLayout,
    taa_resolved_views: &[wgpu::TextureView; 2],
    hdr_fog_view: &wgpu::TextureView,
    screen_w: u32,
    screen_h: u32,
    knee: f32,
    radius: f32,
) -> BloomChain {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("bloom_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });

    let mut mips: Vec<BloomMip> = Vec::with_capacity(BLOOM_MIP_COUNT);
    for level in 0..BLOOM_MIP_COUNT {
        let div = 1u32 << (level + 1);
        let w = (screen_w / div).max(1);
        let h = (screen_h / div).max(1);

        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom_mip"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        mips.push(BloomMip {
            view,
            _tex: tex,
            size: (w, h),
        });
    }

    let prefilter_uniform = make_bloom_uniform(
        device,
        screen_w,
        screen_h,
        mips[0].size.0,
        mips[0].size.1,
        1.0,
        knee,
        radius,
    );

    // ИЗМЕНЕНО (#6): три prefilter bind group.
    let prefilter_bgs: [wgpu::BindGroup; 3] = std::array::from_fn(|i| {
        let src_view: &wgpu::TextureView = if i < 2 {
            &taa_resolved_views[i]
        } else {
            hdr_fog_view
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom_prefilter_bg"),
            layout: bloom_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: prefilter_uniform.as_entire_binding(),
                },
            ],
        })
    });

    let mut downsample_bgs = Vec::with_capacity(BLOOM_MIP_COUNT - 1);
    let mut downsample_uniforms = Vec::with_capacity(BLOOM_MIP_COUNT - 1);
    for i in 1..BLOOM_MIP_COUNT {
        let src = &mips[i - 1];
        let dst = &mips[i];
        let u = make_bloom_uniform(
            device,
            src.size.0,
            src.size.1,
            dst.size.0,
            dst.size.1,
            1.0,
            knee,
            radius,
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom_downsample_bg"),
            layout: bloom_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: u.as_entire_binding(),
                },
            ],
        });
        downsample_bgs.push(bg);
        downsample_uniforms.push(u);
    }

    let mut upsample_bgs = Vec::with_capacity(BLOOM_MIP_COUNT - 1);
    let mut upsample_uniforms = Vec::with_capacity(BLOOM_MIP_COUNT - 1);
    for src_level in (1..BLOOM_MIP_COUNT).rev() {
        let src = &mips[src_level];
        let dst = &mips[src_level - 1];
        let u = make_bloom_uniform(
            device,
            src.size.0,
            src.size.1,
            dst.size.0,
            dst.size.1,
            1.0,
            knee,
            radius,
        );
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bloom_upsample_bg"),
            layout: bloom_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&src.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: u.as_entire_binding(),
                },
            ],
        });
        upsample_bgs.push(bg);
        upsample_uniforms.push(u);
    }

    BloomChain {
        mips,
        sampler,
        prefilter_bgs,
        downsample_bgs,
        upsample_bgs,
        prefilter_uniform,
        downsample_uniforms,
        upsample_uniforms,
        screen_size: (screen_w, screen_h),
    }
}

fn make_bloom_uniform(
    device: &wgpu::Device,
    src_w: u32,
    src_h: u32,
    dst_w: u32,
    dst_h: u32,
    threshold: f32,
    knee: f32,
    radius: f32,
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;

    let data = BloomParams {
        texel: [
            1.0 / src_w.max(1) as f32,
            1.0 / src_h.max(1) as f32,
            1.0 / dst_w.max(1) as f32,
            1.0 / dst_h.max(1) as f32,
        ],
        params: [threshold, knee, radius, 0.0],
    };

    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("bloom_uniform"),
        contents: bytemuck::bytes_of(&data),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}