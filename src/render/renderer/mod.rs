//! Рендерер: публичный API, управление ресурсами, диспетчер проходов.

mod gpu_types;
mod passes;
mod size_dep;

pub use gpu_types::{
    GpuLight, GpuPointLight, MeshDraw, ParticleInstance, PostFx, MAX_DIR_LIGHTS, MAX_POINT_LIGHTS,
};

use std::collections::HashMap;
use std::sync::Arc;
use glam::{Mat4, Vec2, Vec3};
use winit::window::Window;

use crate::render::camera::{Camera3D, CameraMode};
use crate::render::csm::{self, CASCADE_COUNT};
use crate::render::decal::{DecalDraw, DecalInstance};
use crate::render::ibl;
use crate::render::line::{LineBuffer, LineVertex};
use crate::render::material::{Material, MaterialRegistry, SamplerDesc};
use crate::render::mesh::{InstanceData, Mesh, Vertex3D};
use crate::render::shadow_cube;
use crate::render::texture::Texture;

use gpu_types::*;
use size_dep::{build_size_dependent, SizeDependent};

pub struct EguiFrameData<'a> {
    pub renderer: &'a mut egui_wgpu::Renderer,
    pub clipped_primitives: Vec<egui::ClippedPrimitive>,
    pub pixels_per_point: f32,
}

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub size: winit::dpi::PhysicalSize<u32>,

    camera_layout: wgpu::BindGroupLayout,
    lights_layout: wgpu::BindGroupLayout,
    shadow_pass_layout: wgpu::BindGroupLayout,
    shadow2_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    material_layout: wgpu::BindGroupLayout,
    ssao_layout: wgpu::BindGroupLayout,
    lighting_layout: wgpu::BindGroupLayout,
    bloom_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    debug_layout: wgpu::BindGroupLayout,
    debug_depth_layout: wgpu::BindGroupLayout,
    skybox_layout: wgpu::BindGroupLayout,
    taa_layout: wgpu::BindGroupLayout,

    volumetric_compute_layout: wgpu::BindGroupLayout,
    volumetric_composite_layout: wgpu::BindGroupLayout,
    volumetric_uniform: wgpu::Buffer,
    volumetric_pipeline: wgpu::ComputePipeline,
    volumetric_composite_pipeline: wgpu::RenderPipeline,

    camera_buffer: wgpu::Buffer,
    camera_bind_group: wgpu::BindGroup,

    lights_buffer: wgpu::Buffer,
    lights_bind_group: wgpu::BindGroup,

    shadow_pass_buffer: wgpu::Buffer,
    shadow_pass_bind_group: wgpu::BindGroup,
    shadow_pass_stride: u64,

    csm_array_view: wgpu::TextureView,
    csm_cascade_views: [wgpu::TextureView; CASCADE_COUNT],
    csm_sampler: wgpu::Sampler,

    /// ИЗМЕНЕНО (#7): единая cube-array текстура (6 × MAX_SHADOW_CUBES слоёв).
    /// Раньше был один cube (6 слоёв) — тени только для первого point light.
    cube_shadow_array_view: wgpu::TextureView,
    /// Face-views для рендера в слои: `[cube][face]`.
    cube_shadow_face_views: [[wgpu::TextureView; 6]; MAX_SHADOW_CUBES],
    cube_shadow_sampler: wgpu::Sampler,

    material_bind_groups: HashMap<String, MaterialGpu>,
    default_material_bind_group: MaterialGpu,

    sampler_cache: HashMap<SamplerDesc, Arc<wgpu::Sampler>>,

    pub skeleton_buffers: HashMap<String, Arc<wgpu::Buffer>>,

    fallback_texture: Texture,
    fallback_mr: Texture,
    fallback_normal: Texture,
    fallback_emissive: Texture,

    gbuffer_pipeline: wgpu::RenderPipeline,
    gbuffer_pipeline_double_sided: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    shadow_pipeline_double_sided: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    particles_pipeline: wgpu::RenderPipeline,
    transparent_pipeline: wgpu::RenderPipeline,
    transparent_pipeline_double_sided: wgpu::RenderPipeline,
    lighting_pipeline: wgpu::RenderPipeline,
    ssao_pipeline: wgpu::RenderPipeline,
    ssao_blur_pipeline: wgpu::RenderPipeline,
    bloom_prefilter_pipeline: wgpu::RenderPipeline,
    bloom_downsample_pipeline: wgpu::RenderPipeline,
    bloom_upsample_pipeline: wgpu::RenderPipeline,
    tonemap_pipeline: wgpu::RenderPipeline,
    fxaa_pipeline: wgpu::RenderPipeline,
    skybox_pipeline: wgpu::RenderPipeline,
    taa_pipeline: wgpu::RenderPipeline,
    decal_pipeline: wgpu::RenderPipeline,

    debug2d_pipeline: wgpu::RenderPipeline,
    debug_depth_pipeline: wgpu::RenderPipeline,
    csm_debug_bind_group: wgpu::BindGroup,

    tonemap_uniform: wgpu::Buffer,
    ssao_uniform: wgpu::Buffer,

    skybox_uniform: wgpu::Buffer,
    skybox_bind_group: wgpu::BindGroup,

    _ssao_noise_tex: wgpu::Texture,
    ssao_noise_view: wgpu::TextureView,

    ibl: Option<ibl::IblResources>,

    sd: SizeDependent,

    instance_buffer: wgpu::Buffer,
    instance_capacity: u64,
    pub line_buffer: LineBuffer,

    particles_instance_buffer: wgpu::Buffer,
    particles_instance_capacity: u64,

    decal_texture_layout: wgpu::BindGroupLayout,
    decal_depth_layout: wgpu::BindGroupLayout,
    decal_depth_bind_group: wgpu::BindGroup,
    decal_texture_bind_groups: HashMap<String, wgpu::BindGroup>,
    decal_instance_buffer: wgpu::Buffer,
    decal_instance_capacity: u64,

    pub meshes: HashMap<String, Mesh>,
    pub materials: MaterialRegistry,
    pub textures: HashMap<String, Texture>,
    default_material: Material,

    skybox_time: f32,

    /// Счётчик кадров TAA. Чётность определяет, в какую из двух
    /// `taa_resolved_views` писать (ping-pong).
    ///
    /// ИЗМЕНЕНО (#27): инкрементируется через флаг `use_taa`
    /// (`taa_strength > 0.01`), не через повторную проверку. При
    /// выключенной TAA счётчик не растёт, `write_idx` остаётся 0, а
    /// `read_idx = 1` — но сам TAA-pass не выполняется, и bloom/
    /// composite переключаются на чтение из `hdr_fog_view` (индекс 2
    /// в `bloom_chain.prefilter_bgs` / `composite_bgs`). См. #6.
    taa_frame_index: u32,
    taa_prev_view_proj: Mat4,
    taa_reset_frames: u32,
    /// Режим камеры в прошлом кадре — нужен для сброса TAA-истории
    /// при переходах Orbit ↔ FPS ↔ Fly.
    taa_prev_camera_mode: Option<CameraMode>,
    /// Jitter (в пикселях) **предыдущего** кадра.
    /// При jitter = 0 (текущая конфигурация) всегда (0, 0).
    taa_prev_jitter_px: Vec2,

    /// ИЗМЕНЕНО (#6): предыдущее значение «включена ли TAA»
    /// (`postfx.taa_strength > 0.01`). Нужно для сброса
    /// `taa_reset_frames` при переключении TAA через UI: иначе после
    /// включения TAA первые 1-2 кадра шейдер прочитает устаревшую
    /// историю (в `taa_resolved_views` лежит resolve, сделанный до
    /// выключения) — визуально это «фантомный» след.
    taa_prev_enabled: bool,

    /// Статистика последнего **успешно отрисованного** кадра.
    /// Обновляется в `render()` после сортировки draws. `App::redraw`
    /// читает эти поля для панели Stats — они отражают предыдущий
    /// кадр (в текущем кадре render() ещё не вызван на момент
    /// построения Stats). Разница в 1 кадр визуально незаметна.
    pub last_draw_count: usize,
    pub last_instance_count: usize,
}

fn draw_center(d: &MeshDraw) -> Vec3 {
    if d.instances.is_empty() { return Vec3::ZERO; }
    let mut sum = Vec3::ZERO;
    for inst in &d.instances {
        let m = &inst.model;
        sum += Vec3::new(m[3][0], m[3][1], m[3][2]);
    }
    sum / d.instances.len() as f32
}

fn sort_draws_for_render(draws: &[MeshDraw], cam_pos: Vec3) -> Vec<MeshDraw> {
    let mut v: Vec<MeshDraw> = draws.iter().map(|d| MeshDraw {
        mesh: d.mesh.clone(),
        instances: d.instances.clone(),
        texture: d.texture.clone(),
        blend: d.blend,
        double_sided: d.double_sided,
    }).collect();
    v.sort_by_key(|d| d.blend);
    let blend_start = v.partition_point(|d| !d.blend);
    if blend_start < v.len() {
        v[blend_start..].sort_by(|a, b| {
            let da = draw_center(a).distance_squared(cam_pos);
            let db = draw_center(b).distance_squared(cam_pos);
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    v
}

// ИЗМЕНЕНО (#12): удалены `radical_inverse` и `halton_jitter_pixels`.
//
// Обе функции использовались для Halton-последовательности, которая
// давала суб-пиксельный jitter камеры в TAA. Когда jitter был
// отключён (`let jitter_pixels = Vec2::ZERO;`), они перестали
// вызываться и держались только через `let _ = halton_jitter_pixels;`
// — заглушку «не удалять, вдруг пригодится».
//
// Заглушка опасна тем, что скрывает реальное состояние: `cargo
// build` не жалуется, а тот, кто вернёт jitter обратно, не найдёт
// намёка на то, что что-то было сломано. Если jitter понадобится
// снова — восстановить функции из истории git вместе с сопутствующим
// ремонтом motion vectors (см. комментарий в `render()` ниже).

impl Renderer {
    pub async fn new(window: Arc<Window>) -> Self {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }).await.unwrap();

        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits {
                max_vertex_attributes: 32,
                ..wgpu::Limits::default()
            },
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }).await.unwrap();

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 1,
        };
        surface.configure(&device, &config);

        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let lights_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lights_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let shadow_pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow_pass_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<LightsUniform>() as u64,
                    ),
                },
                count: None,
            }],
        });

        let shadow2_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow2_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
                // ИЗМЕНЕНО (#7): Cube → CubeArray (до MAX_SHADOW_CUBES теней).
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::CubeArray, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 9, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 10, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false }, count: None },
            ],
        });

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });

        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<SkeletonUniform>() as u64) },
                    count: None },
            ],
        });

        let ssao_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ssao_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            ],
        });

        let lighting_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lighting_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let bloom_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bloom_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let debug_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let debug_depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("debug_depth_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });

        let skybox_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("skybox_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let taa_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("taa_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let volumetric_compute_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volumetric_compute_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison), count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format: VOLUMETRIC_FORMAT,
                        view_dimension: wgpu::TextureViewDimension::D3 }, count: None },
            ],
        });

        let volumetric_composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("volumetric_composite_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 7, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 8, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let decal_texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decal_texture_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });

        let decal_depth_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("decal_depth_layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });

        let camera_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera_buffer"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let camera_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera_bind_group"),
            layout: &camera_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() }],
        });

        let lights_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lights_buffer"),
            size: std::mem::size_of::<LightsUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lights_bind_group"),
            layout: &lights_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: lights_buffer.as_entire_binding() }],
        });

        let align = device.limits().min_uniform_buffer_offset_alignment as u64;
        let light_size = std::mem::size_of::<LightsUniform>() as u64;
        let shadow_pass_stride = light_size.div_ceil(align) * align;

        let shadow_pass_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shadow_pass_buffer"),
            size: shadow_pass_stride * SHADOW_SLOT_COUNT,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_pass_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow_pass_bind_group"),
            layout: &shadow_pass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_pass_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(light_size),
                }),
            }],
        });

        let tonemap_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tonemap_uniform"),
            size: std::mem::size_of::<TonemapParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ssao_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ssao_uniform"),
            size: std::mem::size_of::<SsaoUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skybox_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skybox_uniform"),
            size: std::mem::size_of::<SkyboxParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let volumetric_uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("volumetric_uniform"),
            size: std::mem::size_of::<VolumetricParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let csm_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("csm_texture"),
            size: wgpu::Extent3d {
                width: csm::CASCADE_SIZE,
                height: csm::CASCADE_SIZE,
                depth_or_array_layers: CASCADE_COUNT as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let csm_array_view = csm_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("csm_array_view"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            base_array_layer: 0,
            array_layer_count: Some(CASCADE_COUNT as u32),
            ..Default::default()
        });
        let csm_cascade_views: [wgpu::TextureView; CASCADE_COUNT] = std::array::from_fn(|i| {
            csm_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("csm_cascade_view"),
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: i as u32,
                array_layer_count: Some(1),
                ..Default::default()
            })
        });
        let csm_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("csm_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        // ИЗМЕНЕНО (#7): `depth_or_array_layers` = 6 × MAX_SHADOW_CUBES.
        // Layout в памяти: слой `cube * 6 + face`. Грани куба идут
        // в порядке +X, -X, +Y, -Y, +Z, -Z (см. shadow_cube::cube_face_matrices).
        let cube_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cube_shadow_texture"),
            size: wgpu::Extent3d {
                width: shadow_cube::CUBE_SIZE,
                height: shadow_cube::CUBE_SIZE,
                depth_or_array_layers: (6 * MAX_SHADOW_CUBES) as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let cube_shadow_array_view = cube_texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("cube_shadow_array_view"),
            dimension: Some(wgpu::TextureViewDimension::CubeArray),
            base_array_layer: 0,
            array_layer_count: Some((6 * MAX_SHADOW_CUBES) as u32),
            base_mip_level: 0,
            mip_level_count: Some(1),
            ..Default::default()
        });
        let cube_shadow_face_views: [[wgpu::TextureView; 6]; MAX_SHADOW_CUBES] =
            std::array::from_fn(|cube| {
                std::array::from_fn(|face| {
                    cube_texture.create_view(&wgpu::TextureViewDescriptor {
                        label: Some("cube_shadow_face_view"),
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: (cube * 6 + face) as u32,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
                })
            });
        let cube_shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cube_shadow_sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let ibl = ibl::IblResources::load_or_default(&device, &queue, "assets/sky.hdr")
            .expect("IBL init failed");

        let skybox_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skybox_bind_group"),
            layout: &skybox_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: camera_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&ibl.env_cube_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&ibl.env_sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: skybox_uniform.as_entire_binding() },
            ],
        });

        let gbuffer_pipeline = make_gbuffer_pipeline(
            &device, &camera_layout, &material_layout, Some(wgpu::Face::Back),
        ).expect("gbuffer pipeline");
        let gbuffer_pipeline_double_sided = make_gbuffer_pipeline(
            &device, &camera_layout, &material_layout, None,
        ).expect("gbuffer pipeline (double-sided)");

        let shadow_pipeline = make_shadow_pipeline(
            &device, &shadow_pass_layout, &material_layout, Some(wgpu::Face::Back),
        ).expect("shadow pipeline");
        let shadow_pipeline_double_sided = make_shadow_pipeline(
            &device, &shadow_pass_layout, &material_layout, None,
        ).expect("shadow pipeline (double-sided)");

        let line_pipeline = make_line_pipeline(&device, &camera_layout).expect("line pipeline");
        let particles_pipeline = make_particles_pipeline(&device, &camera_layout).expect("particles pipeline");

        let transparent_pipeline = make_transparent_pipeline(
            &device, &camera_layout, &lights_layout, &shadow2_layout, &material_layout,
            Some(wgpu::Face::Back),
        ).expect("transparent pipeline");
        let transparent_pipeline_double_sided = make_transparent_pipeline(
            &device, &camera_layout, &lights_layout, &shadow2_layout, &material_layout, None,
        ).expect("transparent pipeline (double-sided)");

        let lighting_pipeline = make_lighting_pipeline(
            &device, &lighting_layout, &lights_layout, &shadow2_layout,
        ).expect("lighting pipeline");
        let (ssao_pipeline, ssao_blur_pipeline) = make_ssao_pipelines(&device, &ssao_layout).expect("ssao pipelines");
        let (bloom_prefilter_pipeline, bloom_downsample_pipeline, bloom_upsample_pipeline) =
            make_bloom_chain_pipelines(&device, &bloom_layout).expect("bloom chain pipelines");
        let tonemap_pipeline = make_tonemap_pipeline(&device, &config, &tonemap_layout).expect("tonemap pipeline");
        let (debug2d_pipeline, debug_depth_pipeline) = make_debug_pipelines(
            &device, &config, &debug_layout, &debug_depth_layout,
        ).expect("debug pipelines");
        let fxaa_pipeline = make_fxaa_pipeline(&device, &config, &debug_layout).expect("fxaa pipeline");
        let skybox_pipeline = make_skybox_pipeline(&device, &skybox_layout).expect("skybox pipeline");
        let taa_pipeline = make_taa_pipeline(&device, &taa_layout).expect("taa pipeline");

        let decal_pipeline = make_decal_pipeline(
            &device, &camera_layout, &decal_texture_layout, &decal_depth_layout,
        ).expect("decal pipeline");

        let volumetric_pipeline = {
            let src = crate::shader_source!("src/render/shaders/volumetric_fog.wgsl")
                .expect("volumetric_fog.wgsl not found");
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("volumetric_fog_shader"),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("volumetric_pipeline_layout"),
                bind_group_layouts: &[&volumetric_compute_layout],
                push_constant_ranges: &[],
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("volumetric_pipeline"),
                layout: Some(&layout),
                module: &shader,
                entry_point: Some("cs_main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };

        let volumetric_composite_pipeline = {
            let src = crate::shader_source!("src/render/shaders/volumetric_composite.wgsl")
                .expect("volumetric_composite.wgsl not found");
            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("volumetric_composite_shader"),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("volumetric_composite_pipeline_layout"),
                bind_group_layouts: &[&volumetric_composite_layout],
                push_constant_ranges: &[],
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("volumetric_composite_pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: Some(wgpu::BlendState::REPLACE),
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

        let (ssao_noise_tex, ssao_noise_view) = create_noise_texture(&device, &queue);

        let sd = build_size_dependent(
            &device, &config,
            &bloom_layout, &tonemap_layout, &ssao_layout,
            &shadow2_layout, &debug_layout, &lighting_layout, &taa_layout,
            &volumetric_compute_layout, &volumetric_composite_layout,
            &ssao_uniform, &volumetric_uniform,
            &ssao_noise_view,
            &csm_array_view, &csm_sampler,
            &cube_shadow_array_view, &cube_shadow_sampler,
            &camera_buffer, &lights_buffer, &ibl, 0.5, 1.0, &tonemap_uniform,
        );

        let decal_depth_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decal_depth_bg"),
            layout: &decal_depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&sd.gbuffer_depth_view),
            }],
        });

        let csm_debug_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("csm_debug_bg"),
            layout: &debug_depth_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&csm_array_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sd.linear_sampler),
                },
            ],
        });

        const INITIAL_INSTANCE_CAPACITY: u64 = 4096;
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: INITIAL_INSTANCE_CAPACITY * std::mem::size_of::<InstanceData>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let fallback_texture = Texture::white(&device, &queue, &texture_layout).unwrap();
        let fallback_emissive = Texture::from_solid(
            &device, &queue, &texture_layout, [0, 0, 0, 255], "fallback_emissive",
        ).unwrap();
        let fallback_mr = Texture::from_solid_linear(
            &device, &queue, &texture_layout, [255, 255, 0, 255], "fallback_mr",
        ).unwrap();
        let fallback_normal = Texture::from_solid_linear(
            &device, &queue, &texture_layout, [128, 128, 255, 255], "fallback_normal",
        ).unwrap();

        let default_material = Material::default();
        let default_sampler = device.create_sampler(&SamplerDesc::default().to_wgpu());
        let default_material_bind_group = build_object_bind_group(
            &device, &material_layout, &default_material,
            &fallback_texture, &fallback_mr, &fallback_normal, &fallback_emissive,
            &default_sampler, create_identity_skeleton_buffer(&device),
        );

        let line_buffer = LineBuffer::new(&device, 4096);

        const INITIAL_PARTICLES_CAPACITY: u64 = 2048;
        let particles_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles_instance_buffer"),
            size: INITIAL_PARTICLES_CAPACITY * std::mem::size_of::<ParticleInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        const INITIAL_DECAL_CAPACITY: u64 = 256;
        let decal_instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decal_instance_buffer"),
            size: INITIAL_DECAL_CAPACITY * std::mem::size_of::<DecalInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            surface, device, queue, config, size,
            camera_layout, lights_layout, shadow_pass_layout, shadow2_layout,
            texture_layout, material_layout, ssao_layout, lighting_layout,
            bloom_layout, tonemap_layout, debug_layout, debug_depth_layout, skybox_layout,
            taa_layout,
            volumetric_compute_layout, volumetric_composite_layout,
            volumetric_uniform, volumetric_pipeline, volumetric_composite_pipeline,
            camera_buffer, camera_bind_group,
            lights_buffer, lights_bind_group,
            shadow_pass_buffer, shadow_pass_bind_group, shadow_pass_stride,
            csm_array_view, csm_cascade_views, csm_sampler,
            cube_shadow_array_view, cube_shadow_face_views, cube_shadow_sampler,
            material_bind_groups: HashMap::new(),
            default_material_bind_group,
            sampler_cache: HashMap::new(),
            skeleton_buffers: HashMap::new(),
            fallback_texture, fallback_mr, fallback_normal, fallback_emissive,
            gbuffer_pipeline, gbuffer_pipeline_double_sided,
            shadow_pipeline, shadow_pipeline_double_sided,
            line_pipeline, particles_pipeline,
            transparent_pipeline, transparent_pipeline_double_sided,
            lighting_pipeline,
            ssao_pipeline, ssao_blur_pipeline,
            bloom_prefilter_pipeline, bloom_downsample_pipeline, bloom_upsample_pipeline,
            tonemap_pipeline, fxaa_pipeline, skybox_pipeline, taa_pipeline, decal_pipeline,
            debug2d_pipeline, debug_depth_pipeline, csm_debug_bind_group,
            tonemap_uniform, ssao_uniform, skybox_uniform, skybox_bind_group,
            _ssao_noise_tex: ssao_noise_tex, ssao_noise_view,
            ibl: Some(ibl), sd,
            instance_buffer, instance_capacity: INITIAL_INSTANCE_CAPACITY,
            line_buffer,
            particles_instance_buffer, particles_instance_capacity: INITIAL_PARTICLES_CAPACITY,
            decal_texture_layout,
            decal_depth_layout,
            decal_depth_bind_group,
            decal_texture_bind_groups: HashMap::new(),
            decal_instance_buffer,
            decal_instance_capacity: INITIAL_DECAL_CAPACITY,
            meshes: HashMap::new(),
            materials: MaterialRegistry::new(),
            textures: HashMap::new(),
            default_material,
            skybox_time: 0.0,
            taa_frame_index: 0,
            taa_prev_view_proj: Mat4::IDENTITY,
            taa_reset_frames: 2,
            taa_prev_camera_mode: None,
            taa_prev_jitter_px: Vec2::ZERO,
            taa_prev_enabled: true,
            last_draw_count: 0,
            last_instance_count: 0,
        }
    }

    #[cfg(debug_assertions)]
    pub fn reload_shaders(&mut self) -> Result<(), String> {
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);

        let build = || -> Result<BuiltPipelines, String> {
            Ok(BuiltPipelines {
                gbuffer: make_gbuffer_pipeline(&self.device, &self.camera_layout, &self.material_layout, Some(wgpu::Face::Back))?,
                gbuffer_double_sided: make_gbuffer_pipeline(&self.device, &self.camera_layout, &self.material_layout, None)?,
                shadow: make_shadow_pipeline(&self.device, &self.shadow_pass_layout, &self.material_layout, Some(wgpu::Face::Back))?,
                shadow_double_sided: make_shadow_pipeline(&self.device, &self.shadow_pass_layout, &self.material_layout, None)?,
                line: make_line_pipeline(&self.device, &self.camera_layout)?,
                particles: make_particles_pipeline(&self.device, &self.camera_layout)?,
                transparent: make_transparent_pipeline(&self.device, &self.camera_layout, &self.lights_layout, &self.shadow2_layout, &self.material_layout, Some(wgpu::Face::Back))?,
                transparent_double_sided: make_transparent_pipeline(&self.device, &self.camera_layout, &self.lights_layout, &self.shadow2_layout, &self.material_layout, None)?,
                lighting: make_lighting_pipeline(&self.device, &self.lighting_layout, &self.lights_layout, &self.shadow2_layout)?,
                ssao: make_ssao_pipelines(&self.device, &self.ssao_layout)?,
                bloom_chain: make_bloom_chain_pipelines(&self.device, &self.bloom_layout)?,
                tonemap: make_tonemap_pipeline(&self.device, &self.config, &self.tonemap_layout)?,
                debug: make_debug_pipelines(&self.device, &self.config, &self.debug_layout, &self.debug_depth_layout)?,
                fxaa: make_fxaa_pipeline(&self.device, &self.config, &self.debug_layout)?,
                skybox: make_skybox_pipeline(&self.device, &self.skybox_layout)?,
                taa: make_taa_pipeline(&self.device, &self.taa_layout)?,
                decal: make_decal_pipeline(&self.device, &self.camera_layout, &self.decal_texture_layout, &self.decal_depth_layout)?,
                volumetric_composite: {
                    let src = crate::shader_source!("src/render/shaders/volumetric_composite.wgsl")?;
                    let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("volumetric_composite_shader"),
                        source: wgpu::ShaderSource::Wgsl(src.into()),
                    });
                    let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                        label: Some("volumetric_composite_pipeline_layout"),
                        bind_group_layouts: &[&self.volumetric_composite_layout],
                        push_constant_ranges: &[],
                    });
                    self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                        label: Some("volumetric_composite_pipeline"),
                        layout: Some(&layout),
                        vertex: wgpu::VertexState {
                            module: &shader,
                            entry_point: Some("vs_main"),
                            buffers: &[],
                            compilation_options: Default::default(),
                        },
                        fragment: Some(wgpu::FragmentState {
                            module: &shader,
                            entry_point: Some("fs_main"),
                            targets: &[Some(wgpu::ColorTargetState {
                                format: HDR_FORMAT,
                                blend: Some(wgpu::BlendState::REPLACE),
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
                },
            })
        };

        let built = match build() {
            Ok(b) => b,
            Err(msg) => {
                let _ = pollster::block_on(self.device.pop_error_scope());
                return Err(msg);
            }
        };

        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            return Err(format!("{}", err));
        }

        self.gbuffer_pipeline = built.gbuffer;
        self.gbuffer_pipeline_double_sided = built.gbuffer_double_sided;
        self.shadow_pipeline = built.shadow;
        self.shadow_pipeline_double_sided = built.shadow_double_sided;
        self.line_pipeline = built.line;
        self.particles_pipeline = built.particles;
        self.transparent_pipeline = built.transparent;
        self.transparent_pipeline_double_sided = built.transparent_double_sided;
        self.lighting_pipeline = built.lighting;
        self.ssao_pipeline = built.ssao.0;
        self.ssao_blur_pipeline = built.ssao.1;
        self.bloom_prefilter_pipeline = built.bloom_chain.0;
        self.bloom_downsample_pipeline = built.bloom_chain.1;
        self.bloom_upsample_pipeline = built.bloom_chain.2;
        self.tonemap_pipeline = built.tonemap;
        self.debug2d_pipeline = built.debug.0;
        self.debug_depth_pipeline = built.debug.1;
        self.fxaa_pipeline = built.fxaa;
        self.skybox_pipeline = built.skybox;
        self.taa_pipeline = built.taa;
        self.decal_pipeline = built.decal;
        self.volumetric_composite_pipeline = built.volumetric_composite;
        Ok(())
    }

    #[cfg(not(debug_assertions))]
    pub fn reload_shaders(&mut self) -> Result<(), String> {
        Err("hot-reload is only supported in debug builds".into())
    }

    pub fn add_mesh(&mut self, name: impl Into<String>, mesh: Mesh) {
        let name = name.into();
        for (i, lod) in mesh.lods.iter().enumerate() {
            let lod_mesh = Mesh::from_raw_parts(
                &self.device, &lod.vertices, &lod.indices,
                &format!("{}__lod{}", name, i), false,
            );
            self.meshes.insert(format!("{}__lod{}", name, i), lod_mesh);
        }
        self.meshes.insert(name, mesh);
    }

    pub fn mesh_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.meshes.keys().filter(|k| !k.contains("__lod")).cloned().collect();
        v.sort();
        v
    }

    pub fn material_names(&self) -> Vec<String> { self.materials.names() }

    pub fn texture_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.textures.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn texture_size(&self, name: &str) -> Option<(u32, u32)> {
        self.textures.get(name).map(|t| t.size)
    }

    pub fn texture_source_path(&self, name: &str) -> Option<&str> {
        self.textures.get(name)?.source_path.as_deref()
    }

    pub fn remove_texture(&mut self, name: &str) -> bool {
        self.decal_texture_bind_groups.remove(name);
        self.textures.remove(name).is_some()
    }

    pub fn load_texture_rgba(&mut self, name: &str, data: &[u8], width: u32, height: u32) -> anyhow::Result<()> {
        let tex = Texture::from_rgba(&self.device, &self.queue, &self.texture_layout, data, width, height, name)?;
        self.textures.insert(name.to_string(), tex);
        Ok(())
    }

    pub fn load_texture_rgba_linear(&mut self, name: &str, data: &[u8], width: u32, height: u32) -> anyhow::Result<()> {
        let tex = Texture::from_rgba_linear(&self.device, &self.queue, &self.texture_layout, data, width, height, name)?;
        self.textures.insert(name.to_string(), tex);
        Ok(())
    }

    /// Загрузка sRGB-текстуры из файла (albedo, emissive, любые «цветные»
    /// картинки, где значения — воспринимаемый цвет).
    pub fn load_texture(&mut self, name: &str, path: &str) -> anyhow::Result<()> {
        let tex = Texture::from_file(&self.device, &self.queue, &self.texture_layout, path)?;
        self.textures.insert(name.to_string(), tex);
        Ok(())
    }

    /// ИЗМЕНЕНО (#8): загрузка linear-текстуры из файла (normal map,
    /// metallic-roughness, AO, height, маски).
    ///
    /// Отличие от `load_texture` — только формат GPU-текстуры:
    /// `Rgba8Unorm` вместо `Rgba8UnormSrgb`. Аппаратное sRGB-декодирование
    /// отключено.
    pub fn load_texture_linear(&mut self, name: &str, path: &str) -> anyhow::Result<()> {
        let tex = Texture::from_file_linear(&self.device, &self.queue, &self.texture_layout, path)?;
        self.textures.insert(name.to_string(), tex);
        Ok(())
    }

    pub fn load_texture_bytes(&mut self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        let tex = Texture::from_bytes(&self.device, &self.queue, &self.texture_layout, bytes, name)?;
        self.textures.insert(name.to_string(), tex);
        Ok(())
    }

    /// Создать bind group для decal-текстуры (ленивая инициализация).
    pub fn ensure_decal_bg(&mut self, name: &str) {
        if self.decal_texture_bind_groups.contains_key(name) { return; }
        let Some(tex) = self.textures.remove(name) else { return; };
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decal_texture_bg"),
            layout: &self.decal_texture_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&tex.sampler) },
            ],
        });
        self.textures.insert(name.to_string(), tex);
        self.decal_texture_bind_groups.insert(name.to_string(), bg);
    }

    fn ensure_decal_capacity(&mut self, needed: u64) {
        if needed <= self.decal_instance_capacity { return; }
        let new_cap = (self.decal_instance_capacity * 2).max(needed);
        self.decal_instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("decal_instance_buffer"),
            size: new_cap * std::mem::size_of::<DecalInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.decal_instance_capacity = new_cap;
    }

    fn get_sampler(&mut self, desc: &SamplerDesc) -> Arc<wgpu::Sampler> {
        if let Some(s) = self.sampler_cache.get(desc) { return Arc::clone(s); }
        let s = Arc::new(self.device.create_sampler(&desc.to_wgpu()));
        self.sampler_cache.insert(*desc, Arc::clone(&s));
        s
    }

    pub fn add_material(&mut self, name: impl Into<String>, mat: Material) {
        let name = name.into();
        let sampler = self.get_sampler(&mat.sampler);
        let base_tex = mat.base_color_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_texture);
        let mr_tex = mat.metallic_roughness_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_mr);
        let normal_tex = mat.normal_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_normal);
        let emissive_tex = mat.emissive_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_emissive);
        let gpu = build_object_bind_group(
            &self.device, &self.material_layout, &mat,
            base_tex, mr_tex, normal_tex, emissive_tex,
            &sampler, create_identity_skeleton_buffer(&self.device),
        );
        self.materials.insert(name.clone(), mat);
        self.material_bind_groups.insert(name, gpu);
    }

    pub fn has_material(&self, name: &str) -> bool {
        self.material_bind_groups.contains_key(name)
    }

    pub fn add_material_with_skeleton(&mut self, name: &str, mat: Material, skeleton_name: &str) {
        let sampler = self.get_sampler(&mat.sampler);
        let base_tex = mat.base_color_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_texture);
        let mr_tex = mat.metallic_roughness_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_mr);
        let normal_tex = mat.normal_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_normal);
        let emissive_tex = mat.emissive_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_emissive);
        let skeleton_uniform = self.skeleton_buffers.get(skeleton_name).cloned()
            .unwrap_or_else(|| create_identity_skeleton_buffer(&self.device));
        let gpu = build_object_bind_group(
            &self.device, &self.material_layout, &mat,
            base_tex, mr_tex, normal_tex, emissive_tex, &sampler, skeleton_uniform,
        );
        self.materials.insert(name.to_string(), mat);
        self.material_bind_groups.insert(name.to_string(), gpu);
    }

    pub fn update_material(&mut self, name: &str, mat: Material) {
        if self.materials.get(name).is_none() { return; }
        let sampler = self.get_sampler(&mat.sampler);
        let base_tex = mat.base_color_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_texture);
        let mr_tex = mat.metallic_roughness_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_mr);
        let normal_tex = mat.normal_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_normal);
        let emissive_tex = mat.emissive_texture.as_deref().and_then(|n| self.textures.get(n)).unwrap_or(&self.fallback_emissive);
        let skeleton_uniform = self.material_bind_groups.get(name).map(|g| g.skeleton_uniform.clone())
            .unwrap_or_else(|| create_identity_skeleton_buffer(&self.device));
        let gpu = build_object_bind_group(
            &self.device, &self.material_layout, &mat,
            base_tex, mr_tex, normal_tex, emissive_tex, &sampler, skeleton_uniform,
        );
        self.materials.insert(name.to_string(), mat);
        self.material_bind_groups.insert(name.to_string(), gpu);
    }

    pub fn materials_default(&self) -> &Material { &self.default_material }

    pub fn add_skeleton(&mut self, name: impl Into<String>, matrices: &[Mat4]) {
        let name = name.into();
        let data = if matrices.is_empty() { SkeletonUniform::identity() }
                   else { SkeletonUniform::from_matrices(matrices) };
        let buffer = Arc::new(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skeleton_buffer"),
            size: std::mem::size_of::<SkeletonUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        self.queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&data));
        self.skeleton_buffers.insert(name, buffer);
    }

    pub fn update_skeleton(&mut self, name: &str, matrices: &[Mat4]) {
        let Some(buffer) = self.skeleton_buffers.get(name) else { return; };
        let data = if matrices.is_empty() { SkeletonUniform::identity() }
                   else { SkeletonUniform::from_matrices(matrices) };
        self.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&data));
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width == 0 || new_size.height == 0 { return; }
        self.size = new_size;
        self.config.width = new_size.width;
        self.config.height = new_size.height;
        self.surface.configure(&self.device, &self.config);
        let ibl_ref = self.ibl.as_ref().expect("IBL must be initialized");
        self.sd = build_size_dependent(
            &self.device, &self.config,
            &self.bloom_layout, &self.tonemap_layout, &self.ssao_layout,
            &self.shadow2_layout, &self.debug_layout, &self.lighting_layout,
            &self.taa_layout,
            &self.volumetric_compute_layout,
            &self.volumetric_composite_layout,
            &self.ssao_uniform, &self.volumetric_uniform,
            &self.ssao_noise_view,
            &self.csm_array_view, &self.csm_sampler,
            &self.cube_shadow_array_view, &self.cube_shadow_sampler,
            &self.camera_buffer, &self.lights_buffer, ibl_ref, 0.5, 1.0, &self.tonemap_uniform,
        );
        self.decal_depth_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("decal_depth_bg"),
            layout: &self.decal_depth_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&self.sd.gbuffer_depth_view),
            }],
        });
        self.csm_debug_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("csm_debug_bg"),
            layout: &self.debug_depth_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.csm_array_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sd.linear_sampler) },
            ],
        });
        self.taa_reset_frames = 2;
    }

    fn ensure_instance_capacity(&mut self, needed: u64) {
        if needed <= self.instance_capacity { return; }
        let new_cap = (self.instance_capacity * 2).max(needed);
        self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("instance_buffer"),
            size: new_cap * std::mem::size_of::<InstanceData>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity = new_cap;
    }

    fn ensure_particles_capacity(&mut self, needed: u64) {
        if needed <= self.particles_instance_capacity { return; }
        let new_cap = (self.particles_instance_capacity * 2).max(needed);
        self.particles_instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles_instance_buffer"),
            size: new_cap * std::mem::size_of::<ParticleInstance>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.particles_instance_capacity = new_cap;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        camera: &Camera3D,
        draws: &[MeshDraw],
        decals: &[DecalDraw],
        particle_instances: &[ParticleInstance],
        line_vertices: &[LineVertex],
        dir_lights: &[GpuLight],
        point_lights: &[GpuPointLight],
        ambient: [f32; 3],
        postfx: PostFx,
        time: f32,
        egui_data: Option<EguiFrameData<'_>>,
    ) -> Result<(), wgpu::SurfaceError> {
        self.skybox_time = time;
        let w_px = self.config.width.max(1) as f32;
        let h_px = self.config.height.max(1) as f32;

        // Сброс TAA-истории при смене режима камеры (Orbit ↔ FPS ↔ Fly):
        // prev_view_proj и история пикселей относятся к старой проекции,
        // из-за чего первые 1–2 кадра виден ghosting.
        if self.taa_prev_camera_mode != Some(camera.mode) {
            self.taa_reset_frames = 2;
            self.taa_prev_camera_mode = Some(camera.mode);
        }

        // ИЗМЕНЕНО (#6): TAA-проход имеет смысл только при
        // `taa_strength > 0.01`.
        let use_taa = postfx.taa_strength > 0.01;
        if use_taa != self.taa_prev_enabled {
            self.taa_reset_frames = 2;
            self.taa_prev_enabled = use_taa;
        }

        // ИЗМЕНЕНО (#12): jitter полностью отключён, `radical_inverse`
        // и `halton_jitter_pixels` удалены. При включённом jitter'е не
        // получалось добиться стабильной картинки: motion buffer,
        // prev_jitter и TAA-history требуют тонкой согласованности во
        // всех шейдерах и во всех uniform'ах. Любая мелкая
        // рассинхронизация давала видимую тряску на субпиксель. При
        // jitter = 0 тряски нет, при этом TAA всё равно сглаживает шум
        // и края за счёт временного накопления истории.
        //
        // Если понадобится настоящий sub-pixel AA — включать jitter
        // обратно нужно вместе с переработкой motion vectors и
        // clip_history. См. историю git: `halton_jitter_pixels` была
        // удалена именно здесь.
        let jitter_pixels = Vec2::ZERO;

        let jitter_ndc = Vec2::new(jitter_pixels.x * 2.0 / w_px, jitter_pixels.y * 2.0 / h_px);

        let view = camera.view_matrix();
        let proj_unjittered = camera.proj_matrix();
        let mut proj_jittered = proj_unjittered;
        proj_jittered.z_axis.x -= jitter_ndc.x;
        proj_jittered.z_axis.y -= jitter_ndc.y;
        let vp_jittered = proj_jittered * view;
        let vp_unjittered = proj_unjittered * view;

        let camera_uniform = CameraUniform {
            view_proj: vp_jittered.to_cols_array_2d(),
            inv_view_proj: vp_jittered.inverse().to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            inv_view: view.inverse().to_cols_array_2d(),
            camera_pos: camera.position().extend(1.0).to_array(),
            near_far: [camera.near, camera.far, jitter_pixels.x, jitter_pixels.y],
            prev_view_proj: self.taa_prev_view_proj.to_cols_array_2d(),
            screen_size: [w_px, h_px, 1.0 / w_px, 1.0 / h_px],
        };
        self.queue.write_buffer(&self.camera_buffer, 0, bytemuck::bytes_of(&camera_uniform));
        self.taa_prev_view_proj = vp_unjittered;

        let write_idx = (self.taa_frame_index as usize) & 1;
        let reset_flag = if self.taa_reset_frames > 0 { 1.0 } else { 0.0 };
        if self.taa_reset_frames > 0 { self.taa_reset_frames -= 1; }

        let taa_params = TaaParams {
            values: [
                (1.0 - postfx.taa_strength) + postfx.taa_strength * 0.1,
                0.125,
                postfx.taa_sharpening,
                reset_flag,
            ],
            screen: [w_px, h_px, jitter_pixels.x, jitter_pixels.y],
            prev_jitter: [
                self.taa_prev_jitter_px.x,
                self.taa_prev_jitter_px.y,
                0.0,
                0.0,
            ],
        };
        self.queue.write_buffer(&self.sd.taa_uniform, 0, bytemuck::bytes_of(&taa_params));

        self.taa_prev_jitter_px = jitter_pixels;

        // ИЗМЕНЕНО (#27): используем `use_taa` вместо повторной проверки
        // `postfx.taa_strength > 0.01`. Семантически то же, но:
        //   1. Единая точка правды: если порог когда-нибудь изменится
        //      (например, 0.05), поменять нужно в одном месте.
        //   2. При `use_taa = false` счётчик не растёт — `write_idx`
        //      остаётся 0, что согласовано с пропуском TAA-pass и
        //      bypass'ом bloom/composite на `hdr_fog_view`.
        if use_taa {
            self.taa_frame_index = self.taa_frame_index.wrapping_add(1);
        }

        let dir_light_dir = dir_lights.first()
            .map(|l| Vec3::new(l.direction[0], l.direction[1], l.direction[2]))
            .unwrap_or(Vec3::Y);

        let mut dir_packed = [[0.0f32; 4]; 8];
        let dir_count = dir_lights.len().min(MAX_DIR_LIGHTS);
        for i in 0..dir_count {
            dir_packed[i * 2] = dir_lights[i].direction;
            dir_packed[i * 2 + 1] = dir_lights[i].color;
        }
        let mut pt_packed = [[0.0f32; 4]; 32];
        let pt_count = point_lights.len().min(MAX_POINT_LIGHTS);
        for i in 0..pt_count {
            pt_packed[i * 2] = point_lights[i].position;
            pt_packed[i * 2 + 1] = point_lights[i].color;
        }

        // ИЗМЕНЕНО (#7): теней от point light теперь до MAX_SHADOW_CUBES.
        let shadow_point_count = pt_count.min(MAX_SHADOW_CUBES);
        let mut cube_pos_packed = [[0.0f32; 4]; 4];
        for i in 0..shadow_point_count {
            cube_pos_packed[i] = point_lights[i].position;
        }

        let splits = csm::split_distances(camera.near, camera.far.min(200.0), 0.5);
        let cascade_vp = csm::build_cascades(
            view, proj_unjittered, camera.near, camera.far, dir_light_dir, &splits,
        );
        let mut csm_packed = [[[0.0f32; 4]; 4]; CASCADE_COUNT];
        for i in 0..CASCADE_COUNT { csm_packed[i] = cascade_vp[i].to_cols_array_2d(); }

        let ambient_color = [ambient[0], ambient[1], ambient[2], 1.0];
        let misc = [postfx.ibl_strength, 0.0, 0.0, 0.0];
        let fog_params = [postfx.fog_density, postfx.fog_height_base, postfx.fog_height_falloff, 0.0];
        let fog_color = [postfx.fog_color[0], postfx.fog_color[1], postfx.fog_color[2], 0.0];
        let shadow_params = [
            postfx.shadow_bias, postfx.shadow_normal_bias,
            postfx.shadow_fade_start, postfx.shadow_fade_end,
        ];

        let lights_uniform = LightsUniform {
            cascade_vp: csm_packed,
            cascade_splits: [splits[1], splits[2], splits[3], 0.0],
            ambient_color,
            counts: [dir_count as u32, pt_count as u32, shadow_point_count as u32, 0],
            light_view_proj: cascade_vp[0].to_cols_array_2d(),
            misc, fog_params, fog_color, shadow_params,
            dir_lights: dir_packed,
            point_lights: pt_packed,
            cube_shadow_pos: cube_pos_packed,
        };
        self.queue.write_buffer(&self.lights_buffer, 0, bytemuck::bytes_of(&lights_uniform));

        {
            let stride = self.shadow_pass_stride as usize;
            let light_size = std::mem::size_of::<LightsUniform>();
            let mut bytes = vec![0u8; stride * SHADOW_SLOT_COUNT as usize];

            let write_slot = |bytes: &mut Vec<u8>, slot: usize, mat: Mat4| {
                let u = LightsUniform {
                    cascade_vp: csm_packed,
                    cascade_splits: [splits[1], splits[2], splits[3], 0.0],
                    ambient_color,
                    counts: [dir_count as u32, pt_count as u32, shadow_point_count as u32, 0],
                    light_view_proj: mat.to_cols_array_2d(),
                    misc, fog_params, fog_color, shadow_params,
                    dir_lights: dir_packed,
                    point_lights: pt_packed,
                    cube_shadow_pos: cube_pos_packed,
                };
                let src = bytemuck::bytes_of(&u);
                let off = slot * stride;
                bytes[off..off + light_size].copy_from_slice(src);
            };

            for c in 0..CASCADE_COUNT { write_slot(&mut bytes, c, cascade_vp[c]); }

            // ИЗМЕНЕНО (#7): записываем матрицы для всех cube shadow maps.
            for cube in 0..shadow_point_count {
                let light_pos = Vec3::new(
                    point_lights[cube].position[0],
                    point_lights[cube].position[1],
                    point_lights[cube].position[2],
                );
                let range = point_lights[cube].position[3];
                let faces = shadow_cube::cube_face_matrices(light_pos, range);
                for (face, m) in faces.iter().enumerate() {
                    write_slot(&mut bytes, 3 + cube * 6 + face, *m);
                }
            }
            // Неиспользуемые слоты заполняем identity — на случай, если
            // шейдер случайно прочитает их, не будет мусора.
            for cube in shadow_point_count..MAX_SHADOW_CUBES {
                for face in 0..6 {
                    write_slot(&mut bytes, 3 + cube * 6 + face, Mat4::IDENTITY);
                }
            }
            self.queue.write_buffer(&self.shadow_pass_buffer, 0, &bytes);
        }

        self.sd.bloom_chain.update_params(
            &self.queue,
            postfx.bloom_threshold,
            postfx.bloom_knee,
            postfx.bloom_radius,
        );

        let tonemap_params = TonemapParams {
            values: [postfx.bloom_strength, postfx.exposure, self.skybox_time, 0.0],
            effects: [postfx.vignette_strength, postfx.film_grain, postfx.chromatic_aberration, 0.0],
        };
        self.queue.write_buffer(&self.tonemap_uniform, 0, bytemuck::bytes_of(&tonemap_params));

        let skybox_params = SkyboxParams {
            values: [1.0, postfx.fog_density, postfx.fog_height_base, postfx.fog_height_falloff],
            fog_color: [postfx.fog_color[0], postfx.fog_color[1], postfx.fog_color[2], 0.0],
        };
        self.queue.write_buffer(&self.skybox_uniform, 0, bytemuck::bytes_of(&skybox_params));

        let noise_tile_x = self.config.width as f32 / 4.0;
        let noise_tile_y = self.config.height as f32 / 4.0;
        let ssao_data = SsaoUniform {
            proj_scale: [proj_unjittered.x_axis.x, proj_unjittered.y_axis.y, camera.far, postfx.ssao_radius],
            params: [0.025, postfx.ssao_strength, noise_tile_x, noise_tile_y],
            time: [0.0, 0.0, 0.0, 0.0],
            view: view.to_cols_array_2d(),
        };
        self.queue.write_buffer(&self.ssao_uniform, 0, bytemuck::bytes_of(&ssao_data));

        let fog_far = camera.far.min(200.0);
        let vol_data = VolumetricParams {
            grid: [
                VOLUMETRIC_GRID_W as f32, VOLUMETRIC_GRID_H as f32,
                1.0 / VOLUMETRIC_GRID_W as f32, 1.0 / VOLUMETRIC_GRID_H as f32,
            ],
            cam: [camera.near, fog_far, (camera.fov_y * 0.5).tan(), camera.aspect],
            params: [postfx.volumetric_density, postfx.volumetric_scattering, postfx.volumetric_phase_g, 0.0],
            fog_color: [postfx.fog_color[0], postfx.fog_color[1], postfx.fog_color[2], 0.0],
        };
        self.queue.write_buffer(&self.volumetric_uniform, 0, bytemuck::bytes_of(&vol_data));

        let debug_params = DebugParams { mode: [postfx.debug_view as u32, 0, 0, 0] };
        self.queue.write_buffer(&self.sd.debug_uniform, 0, bytemuck::bytes_of(&debug_params));

        let fxaa_params = FxaaParams {
            values: [1.0 / w_px, 1.0 / h_px, postfx.fxaa_strength * (1.0 - 0.5 * postfx.taa_strength), 0.0],
        };
        self.queue.write_buffer(&self.sd.fxaa_uniform, 0, bytemuck::bytes_of(&fxaa_params));

        let sorted_draws = sort_draws_for_render(draws, camera.position());

        self.last_draw_count = sorted_draws.len();
        self.last_instance_count = sorted_draws.iter()
            .map(|d| d.instances.len())
            .sum();

        let total_instances: u64 = sorted_draws.iter().map(|d| d.instances.len() as u64).sum();
        if total_instances > 0 {
            self.ensure_instance_capacity(total_instances);
            let stride = std::mem::size_of::<InstanceData>() as u64;
            let mut offset_bytes: u64 = 0;
            for d in &sorted_draws {
                if d.instances.is_empty() { continue; }
                self.queue.write_buffer(&self.instance_buffer, offset_bytes, bytemuck::cast_slice(&d.instances));
                offset_bytes += d.instances.len() as u64 * stride;
            }
        }

        let total_decals: u64 = decals.iter().map(|d| d.instances.len() as u64).sum();
        if total_decals > 0 {
            self.ensure_decal_capacity(total_decals);
            let stride = std::mem::size_of::<DecalInstance>() as u64;
            let mut offset: u64 = 0;
            for d in decals {
                if d.instances.is_empty() { continue; }
                self.queue.write_buffer(&self.decal_instance_buffer, offset, bytemuck::cast_slice(&d.instances));
                offset += d.instances.len() as u64 * stride;
            }
        }

        self.line_buffer.upload(&self.device, &self.queue, line_vertices);

        if !particle_instances.is_empty() {
            self.ensure_particles_capacity(particle_instances.len() as u64);
            self.queue.write_buffer(&self.particles_instance_buffer, 0, bytemuck::cast_slice(particle_instances));
        }

        let frame = self.surface.get_current_texture()?;
        let swap_view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("encoder"),
        });

        passes::encode_csm_all(self, &mut encoder, &sorted_draws);
        // ИЗМЕНЕНО (#7): рендерим cube shadow для всех активных теней.
        if shadow_point_count > 0 {
            passes::encode_cube_shadow_all(
                self,
                &mut encoder,
                &sorted_draws,
                shadow_point_count as u32,
            );
        }
        passes::encode_gbuffer_pass(self, &mut encoder, &sorted_draws);
        passes::encode_decal_pass(self, &mut encoder, decals);
        passes::encode_ssao_pass(self, &mut encoder);
        passes::encode_ssao_blur_pass(self, &mut encoder);
        passes::encode_lighting_pass(self, &mut encoder);
        passes::encode_skybox_pass(self, &mut encoder);
        passes::encode_forward_pass(self, &mut encoder, line_vertices);
        passes::encode_particles_pass(self, &mut encoder, particle_instances.len() as u32);
        passes::encode_transparent_pass(self, &mut encoder, &sorted_draws);
        passes::encode_volumetric_compute(self, &mut encoder);
        passes::encode_volumetric_composite(self, &mut encoder);

        // ИЗМЕНЕНО (#6): TAA-проход выполняется только при `use_taa`.
        if use_taa {
            passes::encode_taa_pass(self, &mut encoder, write_idx);
        }

        if postfx.debug_view.is_debug() {
            passes::encode_debug_pass(self, &mut encoder, &swap_view, postfx.debug_view);
        } else {
            passes::encode_post_processing(
                self,
                &mut encoder,
                &swap_view,
                write_idx,
                use_taa,
            );
        }

        if let Some(egui_data) = egui_data {
            let screen_descriptor = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [self.config.width, self.config.height],
                pixels_per_point: egui_data.pixels_per_point,
            };

            let user_cmd_bufs = egui_data.renderer.update_buffers(
                &self.device, &self.queue, &mut encoder,
                &egui_data.clipped_primitives, &screen_descriptor,
            );
            self.queue.submit(user_cmd_bufs);

            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui_pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &swap_view,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    })
                    .forget_lifetime();
                egui_data.renderer.render(&mut pass, &egui_data.clipped_primitives, &screen_descriptor);
            }
        }

        self.queue.submit(Some(encoder.finish()));
        frame.present();
        Ok(())
    }
}

struct BuiltPipelines {
    gbuffer: wgpu::RenderPipeline,
    gbuffer_double_sided: wgpu::RenderPipeline,
    shadow: wgpu::RenderPipeline,
    shadow_double_sided: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    particles: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    transparent_double_sided: wgpu::RenderPipeline,
    lighting: wgpu::RenderPipeline,
    ssao: (wgpu::RenderPipeline, wgpu::RenderPipeline),
    bloom_chain: (wgpu::RenderPipeline, wgpu::RenderPipeline, wgpu::RenderPipeline),
    tonemap: wgpu::RenderPipeline,
    debug: (wgpu::RenderPipeline, wgpu::RenderPipeline),
    fxaa: wgpu::RenderPipeline,
    skybox: wgpu::RenderPipeline,
    taa: wgpu::RenderPipeline,
    decal: wgpu::RenderPipeline,
    volumetric_composite: wgpu::RenderPipeline,
}

// ============================================================
// Pipeline builders
// ============================================================

fn make_gbuffer_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    material_layout: &wgpu::BindGroupLayout,
    cull: Option<wgpu::Face>,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shaders/gbuffer.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("gbuffer_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("gbuffer_pipeline_layout"),
        bind_group_layouts: &[camera_layout, material_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("gbuffer_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex3D::layout(), InstanceData::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[
                Some(wgpu::ColorTargetState { format: GBUFFER_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                Some(wgpu::ColorTargetState { format: GBUFFER_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                Some(wgpu::ColorTargetState { format: GBUFFER_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL }),
                Some(wgpu::ColorTargetState { format: MOTION_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL }),
            ],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_shadow_pipeline(
    device: &wgpu::Device,
    shadow_pass_layout: &wgpu::BindGroupLayout,
    material_layout: &wgpu::BindGroupLayout,
    cull: Option<wgpu::Face>,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shadow.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("shadow_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow_pipeline_layout"),
        bind_group_layouts: &[shadow_pass_layout, material_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("shadow_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex3D::layout(), InstanceData::layout()],
            compilation_options: Default::default(),
        },
        fragment: None,
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState { constant: 4, slope_scale: 2.0, clamp: 0.0 },
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_line_pipeline(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shaders/lines.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("line_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("line_pipeline_layout"),
        bind_group_layouts: &[camera_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("line_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[LineVertex::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            front_face: wgpu::FrontFace::Ccw,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_particles_pipeline(device: &wgpu::Device, camera_layout: &wgpu::BindGroupLayout) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/particles.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("particles_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("particles_pipeline_layout"),
        bind_group_layouts: &[camera_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("particles_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[ParticleInstance::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_transparent_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    lights_layout: &wgpu::BindGroupLayout,
    shadow2_layout: &wgpu::BindGroupLayout,
    material_layout: &wgpu::BindGroupLayout,
    cull: Option<wgpu::Face>,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shaders/forward_transparent.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("transparent_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("transparent_pipeline_layout"),
        bind_group_layouts: &[camera_layout, lights_layout, shadow2_layout, material_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("transparent_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex3D::layout(), InstanceData::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: MOTION_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
            ],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_lighting_pipeline(
    device: &wgpu::Device,
    lighting_layout: &wgpu::BindGroupLayout,
    lights_layout: &wgpu::BindGroupLayout,
    shadow2_layout: &wgpu::BindGroupLayout,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shaders/deferred_lighting.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("lighting_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("lighting_pipeline_layout"),
        bind_group_layouts: &[lighting_layout, lights_layout, shadow2_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("lighting_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
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
    }))
}

fn make_ssao_pipelines(
    device: &wgpu::Device,
    ssao_layout: &wgpu::BindGroupLayout,
) -> Result<(wgpu::RenderPipeline, wgpu::RenderPipeline), String> {
    let src = crate::shader_source!("src/render/ssao.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ssao_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ssao_pipeline_layout"),
        bind_group_layouts: &[ssao_layout],
        push_constant_ranges: &[],
    });
    let main = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ssao_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_ssao"),
            targets: &[Some(wgpu::ColorTargetState {
                format: SSAO_FORMAT,
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
    });
    let blur = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ssao_blur_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_blur"),
            targets: &[Some(wgpu::ColorTargetState {
                format: SSAO_FORMAT,
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
    });
    Ok((main, blur))
}

fn make_bloom_chain_pipelines(
    device: &wgpu::Device,
    bloom_layout: &wgpu::BindGroupLayout,
) -> Result<(wgpu::RenderPipeline, wgpu::RenderPipeline, wgpu::RenderPipeline), String> {
    let src = crate::shader_source!("src/render/bloom.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("bloom_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("bloom_pipeline_layout"),
        bind_group_layouts: &[bloom_layout],
        push_constant_ranges: &[],
    });

    let make = |entry: &str, blend: Option<wgpu::BlendState>| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(entry),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(entry),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend,
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

    let prefilter = make("fs_prefilter", None);
    let downsample = make("fs_downsample", None);
    let upsample_blend = wgpu::BlendState {
        color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add },
        alpha: wgpu::BlendComponent::OVER,
    };
    let upsample = make("fs_upsample", Some(upsample_blend));
    Ok((prefilter, downsample, upsample))
}

fn make_tonemap_pipeline(
    device: &wgpu::Device,
    _config: &wgpu::SurfaceConfiguration,
    tonemap_layout: &wgpu::BindGroupLayout,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/tonemap.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("tonemap_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("tonemap_pipeline_layout"),
        bind_group_layouts: &[tonemap_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("tonemap_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: LDR_FORMAT,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_fxaa_pipeline(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    fxaa_layout: &wgpu::BindGroupLayout,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/fxaa.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fxaa_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("fxaa_pipeline_layout"),
        bind_group_layouts: &[fxaa_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fxaa_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: config.format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_skybox_pipeline(device: &wgpu::Device, skybox_layout: &wgpu::BindGroupLayout) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/skybox.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("skybox_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("skybox_pipeline_layout"),
        bind_group_layouts: &[skybox_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("skybox_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::LessEqual,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_taa_pipeline(device: &wgpu::Device, taa_layout: &wgpu::BindGroupLayout) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shaders/taa.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("taa_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("taa_pipeline_layout"),
        bind_group_layouts: &[taa_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("taa_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
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
    }))
}

fn make_decal_pipeline(
    device: &wgpu::Device,
    camera_layout: &wgpu::BindGroupLayout,
    decal_texture_layout: &wgpu::BindGroupLayout,
    decal_depth_layout: &wgpu::BindGroupLayout,
) -> Result<wgpu::RenderPipeline, String> {
    let src = crate::shader_source!("src/render/shaders/decal.wgsl")?;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("decal_shader"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("decal_pipeline_layout"),
        bind_group_layouts: &[camera_layout, decal_texture_layout, decal_depth_layout],
        push_constant_ranges: &[],
    });
    Ok(device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("decal_pipeline"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex3D::layout(), DecalInstance::layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[
                Some(wgpu::ColorTargetState {
                    format: GBUFFER_FORMAT,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                }),
                Some(wgpu::ColorTargetState {
                    format: GBUFFER_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                }),
                Some(wgpu::ColorTargetState {
                    format: GBUFFER_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                }),
                Some(wgpu::ColorTargetState {
                    format: MOTION_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::empty(),
                }),
            ],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    }))
}

fn make_debug_pipelines(
    device: &wgpu::Device,
    config: &wgpu::SurfaceConfiguration,
    debug_layout: &wgpu::BindGroupLayout,
    debug_depth_layout: &wgpu::BindGroupLayout,
) -> Result<(wgpu::RenderPipeline, wgpu::RenderPipeline), String> {
    let src2d = crate::shader_source!("src/render/shaders/debug2d.wgsl")?;
    let src_depth = crate::shader_source!("src/render/shaders/debug_depth.wgsl")?;
    let shader2d = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("debug2d_shader"),
        source: wgpu::ShaderSource::Wgsl(src2d.into()),
    });
    let shader_depth = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("debug_depth_shader"),
        source: wgpu::ShaderSource::Wgsl(src_depth.into()),
    });

    let layout2d = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("debug2d_layout"),
        bind_group_layouts: &[debug_layout],
        push_constant_ranges: &[],
    });
    let layout_depth = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("debug_depth_layout"),
        bind_group_layouts: &[debug_depth_layout],
        push_constant_ranges: &[],
    });

    let pipeline2d = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("debug2d_pipeline"),
        layout: Some(&layout2d),
        vertex: wgpu::VertexState {
            module: &shader2d,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader2d,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: config.format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    let pipeline_depth = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("debug_depth_pipeline"),
        layout: Some(&layout_depth),
        vertex: wgpu::VertexState {
            module: &shader_depth,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader_depth,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: config.format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    });

    Ok((pipeline2d, pipeline_depth))
}