//! Утилиты для click-to-place: пересечение луча с плоскостью земли.

use glam::Vec3;

/// Пересечение луча с горизонтальной плоскостью y = plane_y.
/// Возвращает точку попадания или None, если луч параллелен
/// или идёт в противоположную сторону.
pub fn ray_ground_plane(origin: Vec3, dir: Vec3, plane_y: f32) -> Option<Vec3> {
    if dir.y.abs() < 1e-6 {
        return None;
    }
    let t = (plane_y - origin.y) / dir.y;
    if t < 0.0 {
        return None;
    }
    Some(origin + dir * t)
}

/// Snap к сетке с шагом `step`. Для Y — тоже snap, но по высоте
/// примитив приподнимается отдельно.
pub fn snap_to_grid(p: Vec3, step: f32) -> Vec3 {
    if step <= 1e-6 {
        return p;
    }
    Vec3::new(
        (p.x / step).round() * step,
        p.y,
        (p.z / step).round() * step,
    )
}