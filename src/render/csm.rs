//! Каскадные shadow maps для directional light.
//! 3 каскада: ближний (высокое разрешение), средний, дальний.

use glam::{Mat4, Vec3, Vec4};

pub const CASCADE_COUNT: usize = 3;
pub const CASCADE_SIZE: u32 = 2048;

/// Разбиение дальности камеры на каскады.
pub fn split_distances(near: f32, far: f32, lambda: f32) -> [f32; CASCADE_COUNT + 1] {
    let mut result = [0.0f32; CASCADE_COUNT + 1];
    result[0] = near;
    for i in 1..=CASCADE_COUNT {
        let p = i as f32 / CASCADE_COUNT as f32;
        let log = near * (far / near).powf(p);
        let uni = near + (far - near) * p;
        result[i] = lambda * log + (1.0 - lambda) * uni;
    }
    result
}

/// Построить view-projection для каждого каскада.
///
/// `camera_near`/`camera_far` — near/far плоскость **исходной** камеры.
/// Они нужны, чтобы вычислить корректные NDC-z для под-frustum'а
/// (в wgpu NDC-z ∈ [0, 1] нелинейно связан с расстоянием).
pub fn build_cascades(
    camera_view: Mat4,
    camera_proj: Mat4,
    camera_near: f32,
    camera_far: f32,
    sun_dir: Vec3,
    splits: &[f32; CASCADE_COUNT + 1],
) -> [Mat4; CASCADE_COUNT] {
    let inv_view = camera_view.inverse();
    let inv_proj = camera_proj.inverse();

    let mut result = [Mat4::IDENTITY; CASCADE_COUNT];

    for i in 0..CASCADE_COUNT {
        let near_i = splits[i];
        let far_i = splits[i + 1];

        // Корректные NDC-z для near_i и far_i.
        // Формула для perspective_rh из glam:
        //   ndc_z(d) = far * (d - near) / (d * (far - near))
        let ndc_z_near = if near_i > camera_near {
            camera_far * (near_i - camera_near) / (near_i * (camera_far - camera_near))
        } else {
            0.0
        };
        let ndc_z_far = if far_i > camera_near {
            camera_far * (far_i - camera_near) / (far_i * (camera_far - camera_near))
        } else {
            1.0
        };

        // 8 углов под-frustum в NDC
        let corners_ndc = [
            Vec4::new(-1.0, -1.0, ndc_z_near, 1.0),
            Vec4::new( 1.0, -1.0, ndc_z_near, 1.0),
            Vec4::new(-1.0,  1.0, ndc_z_near, 1.0),
            Vec4::new( 1.0,  1.0, ndc_z_near, 1.0),
            Vec4::new(-1.0, -1.0, ndc_z_far,  1.0),
            Vec4::new( 1.0, -1.0, ndc_z_far,  1.0),
            Vec4::new(-1.0,  1.0, ndc_z_far,  1.0),
            Vec4::new( 1.0,  1.0, ndc_z_far,  1.0),
        ];

        // Мировые координаты углов
        let mut corners_world = [Vec3::ZERO; 8];
        for (j, c) in corners_ndc.iter().enumerate() {
            let world = inv_view * inv_proj * *c;
            corners_world[j] = world.truncate() / world.w;
        }

        // Центр и радиус сферы, покрывающей все 8 углов
        let mut center = Vec3::ZERO;
        for c in &corners_world {
            center += *c;
        }
        center /= 8.0;

        let mut radius = 0.0f32;
        for c in &corners_world {
            radius = radius.max((*c - center).length());
        }
        // Округлим радиус вверх для стабильности
        let radius = (radius * 16.0).ceil() / 16.0;

        // Позиция источника света — сдвигаем центр в сторону солнца
        let sun = sun_dir.normalize_or_zero();
        let eye = center + sun * radius * 2.0;

        let view = Mat4::look_at_rh(eye, center, Vec3::Y);
        let proj = Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.1, radius * 4.0);

        result[i] = proj * view;
    }

    result
}