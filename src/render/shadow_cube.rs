use glam::{Mat4, Vec3};

/// Размер одной грани cube shadow map.
pub const CUBE_SIZE: u32 = 1024;

/// 6 view-projection матриц для рендера в cube map.
/// **Ортографическая** проекция — потому что в шейдере мы сравниваем
/// линейную нормированную дистанцию `dist/range`, а не NDC-z.
/// Порядок: +X, -X, +Y, -Y, +Z, -Z.
pub fn cube_face_matrices(light_pos: Vec3, range: f32) -> [Mat4; 6] {
    // ortho: NDC-z = dist/range, линейно.
    let proj = Mat4::orthographic_rh(-range, range, -range, range, 0.0, range);

    let targets = [
        (Vec3::new( 1.0, 0.0, 0.0), Vec3::new(0.0, -1.0, 0.0)),
        (Vec3::new(-1.0, 0.0, 0.0), Vec3::new(0.0, -1.0, 0.0)),
        (Vec3::new(0.0,  1.0, 0.0), Vec3::new(0.0, 0.0,  1.0)),
        (Vec3::new(0.0, -1.0, 0.0), Vec3::new(0.0, 0.0, -1.0)),
        (Vec3::new(0.0, 0.0,  1.0), Vec3::new(0.0, -1.0, 0.0)),
        (Vec3::new(0.0, 0.0, -1.0), Vec3::new(0.0, -1.0, 0.0)),
    ];

    let mut result = [Mat4::IDENTITY; 6];
    for (i, (dir, up)) in targets.iter().enumerate() {
        let view = Mat4::look_at_rh(light_pos, light_pos + *dir, *up);
        result[i] = proj * view;
    }
    result
}

pub struct CubeShadow {
    pub texture: wgpu::Texture,
    pub face_views: [wgpu::TextureView; 6],
    pub cube_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

pub fn create_cube_shadow(device: &wgpu::Device) -> CubeShadow {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cube_shadow"),
        size: wgpu::Extent3d {
            width: CUBE_SIZE,
            height: CUBE_SIZE,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });

    let face_views: [wgpu::TextureView; 6] = std::array::from_fn(|i| {
        texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some(&format!("cube_shadow_face_{i}")),
            dimension: Some(wgpu::TextureViewDimension::D2),
            base_array_layer: i as u32,
            array_layer_count: Some(1),
            ..Default::default()
        })
    });

    let cube_view = texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("cube_shadow_cube"),
        dimension: Some(wgpu::TextureViewDimension::Cube),
        base_array_layer: 0,
        array_layer_count: Some(6),
        ..Default::default()
    });

    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("cube_shadow_sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });

    CubeShadow {
        texture,
        face_views,
        cube_view,
        sampler,
    }
}