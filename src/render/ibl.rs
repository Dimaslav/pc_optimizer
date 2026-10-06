//! IBL (Image-Based Lighting).

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use std::path::Path;

pub const ENV_SIZE: u32 = 512;
pub const IRRADIANCE_SIZE: u32 = 32;
pub const PREFILTER_SIZE: u32 = 256;
pub const PREFILTER_MIPS: u32 = 5;
pub const BRDF_LUT_SIZE: u32 = 256;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct FaceUniform {
    right: [f32; 4],
    up: [f32; 4],
    forward: [f32; 4],
    params: [f32; 4],
}

const FACE_DIRS: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
    ([0.0, 0.0, -1.0], [0.0, -1.0, 0.0], [1.0, 0.0, 0.0]),   // +X
    ([0.0, 0.0, 1.0], [0.0, -1.0, 0.0], [-1.0, 0.0, 0.0]),    // -X
    ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),      // +Y
    ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, -1.0, 0.0]),    // -Y
    ([1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),     // +Z
    ([-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]),   // -Z
];

pub struct IblResources {
    pub env_cube_view: wgpu::TextureView,
    pub irradiance_view: wgpu::TextureView,
    pub prefiltered_view: wgpu::TextureView,
    pub brdf_lut_view: wgpu::TextureView,
    pub env_sampler: wgpu::Sampler,
    _env_tex: wgpu::Texture,
    _irr_tex: wgpu::Texture,
    _pre_tex: wgpu::Texture,
    _brdf_tex: wgpu::Texture,
}

impl IblResources {
    pub fn load_or_default(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        let path_ref = path.as_ref();
        if !path_ref.exists() {
            log::warn!(
                "IBL: '{}' не найден → процедурное небо",
                path_ref.display()
            );
            return Self::procedural(device, queue);
        }
        match Self::load(device, queue, path_ref) {
            Ok(r) => {
                log::info!("IBL: загружено небо из '{}'", path_ref.display());
                Ok(r)
            }
            Err(e) => {
                log::warn!(
                    "IBL: HDRI не загружена ({}), процедурный fallback",
                    e
                );
                Self::procedural(device, queue)
            }
        }
    }

    pub fn load(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        path: impl AsRef<Path>,
    ) -> Result<Self> {
        let path = path.as_ref();

        let img = image::open(path)
            .with_context(|| format!("failed to open HDRI: {}", path.display()))?
            .to_rgb32f();
        let (w, h) = img.dimensions();
        let data: Vec<f32> = img.into_raw();

        let mut rgba: Vec<f32> = Vec::with_capacity(data.len() / 3 * 4);
        for px in data.chunks_exact(3) {
            rgba.push(px[0]);
            rgba.push(px[1]);
            rgba.push(px[2]);
            rgba.push(1.0);
        }

        let equirect_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("equirect_hdr"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &equirect_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&rgba),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 16),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );

        let equirect_view = equirect_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let equirect_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("equirect_sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let (env_tex, env_views, env_view_all) =
            create_cubemap(device, "env_cube", ENV_SIZE, 1);
        let (irr_tex, irr_views, irr_view_all) =
            create_cubemap(device, "irradiance", IRRADIANCE_SIZE, 1);
        let (pre_tex, pre_views, pre_view_all) =
            create_cubemap(device, "prefiltered", PREFILTER_SIZE, PREFILTER_MIPS);

        let brdf_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brdf_lut"),
            size: wgpu::Extent3d {
                width: BRDF_LUT_SIZE,
                height: BRDF_LUT_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let brdf_view = brdf_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let equirect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("equirect_to_cube"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/equirect_to_cube.wgsl").into(),
            ),
        });
        let irr_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("irradiance"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/irradiance.wgsl").into()),
        });
        let pre_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("prefilter"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/prefilter.wgsl").into()),
        });
        let brdf_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("brdf_lut"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/brdf_lut.wgsl").into()),
        });

        let face_bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("face_bg_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let equirect_bg_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("equirect_bg_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let make_pipeline = |shader: &wgpu::ShaderModule,
                              layout: &wgpu::BindGroupLayout,
                              entry: &str,
                              format: wgpu::TextureFormat|
         -> wgpu::RenderPipeline {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };

        let hdr_cube_format = wgpu::TextureFormat::Rgba16Float;
        let equirect_pipeline =
            make_pipeline(&equirect_shader, &equirect_bg_layout, "fs_main", hdr_cube_format);
        let irr_pipeline =
            make_pipeline(&irr_shader, &face_bg_layout, "fs_main", hdr_cube_format);
        let pre_pipeline =
            make_pipeline(&pre_shader, &face_bg_layout, "fs_main", hdr_cube_format);
        let brdf_pipeline = make_pipeline(
            &brdf_shader,
            &face_bg_layout,
            "fs_main",
            wgpu::TextureFormat::Rg16Float,
        );

        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("env_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("ibl_encoder"),
        });

        for face in 0..6 {
            let uniform = make_face_uniform(face, 0.0);
            let buf = create_uniform_buffer(device, &uniform);
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &equirect_bg_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&equirect_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&equirect_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buf.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &env_views[face],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&equirect_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        for face in 0..6 {
            let uniform = make_face_uniform(face, 0.0);
            let buf = create_uniform_buffer(device, &uniform);
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &face_bg_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&env_view_all),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&env_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buf.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &irr_views[face],
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&irr_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        for mip in 0..PREFILTER_MIPS {
            let roughness = mip as f32 / (PREFILTER_MIPS - 1).max(1) as f32;
            for face in 0..6usize {
                let uniform = make_face_uniform(face, roughness);
                let buf = create_uniform_buffer(device, &uniform);
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &face_bg_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&env_view_all),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&env_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: buf.as_entire_binding(),
                        },
                    ],
                });
                let view = &pre_views[mip as usize * 6 + face];
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&pre_pipeline);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
        }

        {
            let dummy_tex = device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 6 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let dummy_view = dummy_tex.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                base_array_layer: 0,
                array_layer_count: Some(6),
                ..Default::default()
            });
            let dummy_buf = create_uniform_buffer(device, &FaceUniform::zeroed());
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &face_bg_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&dummy_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&env_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: dummy_buf.as_entire_binding(),
                    },
                ],
            });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &brdf_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&brdf_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }

        queue.submit(Some(encoder.finish()));

        Ok(IblResources {
            env_cube_view: env_view_all,
            irradiance_view: irr_view_all,
            prefiltered_view: pre_view_all,
            brdf_lut_view: brdf_view,
            env_sampler,
            _env_tex: env_tex,
            _irr_tex: irr_tex,
            _pre_tex: pre_tex,
            _brdf_tex: brdf_tex,
        })
    }

    /// Fallback: 1×1 cubemap с цветом неба.
    ///
    /// Значения — HDR (до ACES tonemap). Умеренные, чтобы:
    ///   1. Skybox после ACES выглядел светло-голубым, НЕ белым.
    ///   2. IBL-diffuse (ambient) не выжигал всю сцену.
    ///
    /// Если увеличить — вся сцена станет белёсой; если уменьшить —
    /// небо станет тёмным и сцена уйдёт в тень.
    fn procedural(device: &wgpu::Device, queue: &wgpu::Queue) -> Result<Self> {
        fn f32_to_f16(v: f32) -> u16 {
            let bits = v.to_bits();
            let sign = ((bits >> 31) & 0x1) as u16;
            let exp = ((bits >> 23) & 0xFF) as i32 - 127 + 15;
            let mant = (bits >> 13) & 0x3FF;
            if exp <= 0 {
                return sign << 15;
            }
            if exp >= 31 {
                return (sign << 15) | 0x7C00;
            }
            (sign << 15) | ((exp as u16) << 10) | (mant as u16)
        }

        let make_cube = |device: &wgpu::Device, label: &str| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };

        let env_tex = make_cube(device, "env_dummy");
        let irr_tex = make_cube(device, "irr_dummy");
        let pre_tex = make_cube(device, "pre_dummy");

        // Умеренно-голубое небо. После ACES даст светло-голубой,
        // не белый. IBL-diffuse от этих значений не выжигает albedo.
        // Было (0.8, 1.0, 1.3) → пересвет всего в белый.
        let (r, g, b) = (0.35_f32, 0.5_f32, 0.75_f32);

        let mut sky_px = Vec::with_capacity(8);
        sky_px.extend_from_slice(&f32_to_f16(r).to_le_bytes());
        sky_px.extend_from_slice(&f32_to_f16(g).to_le_bytes());
        sky_px.extend_from_slice(&f32_to_f16(b).to_le_bytes());
        sky_px.extend_from_slice(&f32_to_f16(1.0).to_le_bytes());

        for face in 0..6u32 {
            for tex in [&env_tex, &irr_tex, &pre_tex] {
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: 0, y: 0, z: face },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &sky_px,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(8),
                        rows_per_image: Some(1),
                    },
                    wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }

        let brdf_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("brdf_dummy"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rg16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let brdf_px = [
            f32_to_f16(1.0).to_le_bytes()[0],
            f32_to_f16(1.0).to_le_bytes()[1],
            f32_to_f16(0.0).to_le_bytes()[0],
            f32_to_f16(0.0).to_le_bytes()[1],
        ];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &brdf_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &brdf_px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );

        let env_cube_view = env_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            base_array_layer: 0,
            array_layer_count: Some(6),
            ..Default::default()
        });
        let irr_view = irr_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            base_array_layer: 0,
            array_layer_count: Some(6),
            ..Default::default()
        });
        let pre_view = pre_tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            base_array_layer: 0,
            array_layer_count: Some(6),
            ..Default::default()
        });
        let brdf_view = brdf_tex.create_view(&wgpu::TextureViewDescriptor::default());

        let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("env_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(IblResources {
            env_cube_view,
            irradiance_view: irr_view,
            prefiltered_view: pre_view,
            brdf_lut_view: brdf_view,
            env_sampler,
            _env_tex: env_tex,
            _irr_tex: irr_tex,
            _pre_tex: pre_tex,
            _brdf_tex: brdf_tex,
        })
    }
}

fn make_face_uniform(face: usize, roughness: f32) -> FaceUniform {
    let (r, u, f) = FACE_DIRS[face];
    FaceUniform {
        right: [r[0], r[1], r[2], 0.0],
        up: [u[0], u[1], u[2], 0.0],
        forward: [f[0], f[1], f[2], 0.0],
        params: [roughness, 0.0, 0.0, 0.0],
    }
}

fn create_uniform_buffer(device: &wgpu::Device, data: &FaceUniform) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("face_uniform"),
        contents: bytemuck::bytes_of(data),
        usage: wgpu::BufferUsages::UNIFORM,
    })
}

fn create_cubemap(
    device: &wgpu::Device,
    label: &str,
    size: u32,
    mips: u32,
) -> (wgpu::Texture, Vec<wgpu::TextureView>, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let mut face_views = Vec::with_capacity((mips * 6) as usize);
    for mip in 0..mips {
        for face in 0..6 {
            face_views.push(texture.create_view(&wgpu::TextureViewDescriptor {
                label: None,
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: mip,
                mip_level_count: Some(1),
                base_array_layer: face,
                array_layer_count: Some(1),
                ..Default::default()
            }));
        }
    }

    let all_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: None,
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_mip_level: 0,
        mip_level_count: Some(mips),
        base_array_layer: 0,
        array_layer_count: Some(6),
        ..Default::default()
    });

    (texture, face_views, all_view)
}