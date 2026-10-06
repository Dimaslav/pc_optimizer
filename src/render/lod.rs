//! Автоматическая генерация LOD-уровней.
//!
//! Алгоритм: vertex clustering.
//! 1. Разбиваем AABB меша на 3D-сетку resolution × resolution × resolution.
//! 2. Все вершины, попавшие в ячейку, «склеиваются» в одну (усреднение
//!    позиции/нормали/UV).
//! 3. Треугольники, у которых две или три вершины попали в одну ячейку,
//!    выбрасываются.
//! 4. Получается грубая, но топологически связная версия.
//!
//! Это быстрый O(n) метод — не даёт такого же качества, как QEM
//! (quadric error metric), но не требует сложных вычислений и
//! достаточен для дистанционных LOD.

use std::collections::HashMap;

use glam::Vec3;

use super::mesh::Vertex3D;

/// Один LOD-уровень: список вершин и индексов.
pub struct LodLevel {
    /// Во сколько раз меньше треугольников, чем в LOD0 (примерно).
    pub ratio: f32,
    /// Разрешение voxel-сетки, использованное при генерации.
    pub resolution: u32,
    pub vertices: Vec<Vertex3D>,
    pub indices: Vec<u32>,
}

/// Сгенерировать LOD-уровни из исходного меша.
///
/// `levels` — сколько уровней сгенерировать (не считая LOD0).
/// Типично 2-3.
///
/// Разрешения растут: 16, 12, 8, 6, 4, 3, 2 — чем меньше,
/// тем грубее меш.
pub fn generate_lods(
    base_vertices: &[Vertex3D],
    base_indices: &[u32],
    levels: usize,
) -> Vec<LodLevel> {
    if base_vertices.is_empty() || base_indices.is_empty() || levels == 0 {
        return Vec::new();
    }

    // Считаем AABB.
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for v in base_vertices {
        let p = Vec3::from(v.position);
        min = min.min(p);
        max = max.max(p);
    }
    let size = (max - min).max_element().max(1e-4);
    let inv_size = 1.0 / size;

    let resolutions = [16u32, 12, 8, 6, 4, 3, 2];

    let base_tri_count = base_indices.len() / 3;
    let mut result = Vec::with_capacity(levels);

    for &res in resolutions.iter().take(levels) {
        // Кластеризация вершин.
        let mut cluster_map: HashMap<u32, u32> = HashMap::new();
        let mut new_vertices: Vec<Vertex3D> = Vec::new();
        let mut sum_pos: Vec<Vec3> = Vec::new();
        let mut sum_nrm: Vec<Vec3> = Vec::new();
        let mut sum_uv: Vec<glam::Vec2> = Vec::new();
        let mut count: Vec<u32> = Vec::new();

        for v in base_vertices.iter() {
            let p = Vec3::from(v.position);
            let norm = (p - min) * inv_size; // [0, 1]
            let cx = ((norm.x * res as f32) as u32).min(res - 1);
            let cy = ((norm.y * res as f32) as u32).min(res - 1);
            let cz = ((norm.z * res as f32) as u32).min(res - 1);
            let key = cx | (cy << 10) | (cz << 20);

            let new_idx = match cluster_map.get(&key) {
                Some(&idx) => idx,
                None => {
                    let idx = new_vertices.len() as u32;
                    cluster_map.insert(key, idx);

                    new_vertices.push(Vertex3D {
                        position: [0.0; 3],
                        normal: [0.0; 3],
                        uv: [0.0; 2],
                        color: v.color,
                        joints: v.joints,
                        weights: v.weights,
                    });
                    sum_pos.push(Vec3::ZERO);
                    sum_nrm.push(Vec3::ZERO);
                    sum_uv.push(glam::Vec2::ZERO);
                    count.push(0);
                    idx
                }
            };

            sum_pos[new_idx as usize] += p;
            sum_nrm[new_idx as usize] += Vec3::from(v.normal);
            sum_uv[new_idx as usize] += glam::Vec2::from(v.uv);
            count[new_idx as usize] += 1;
        }

        // Финализация усреднённых вершин.
        for i in 0..new_vertices.len() {
            let c = count[i].max(1) as f32;
            let pos = sum_pos[i] / c;
            let nrm = if sum_nrm[i].length_squared() > 1e-8 {
                sum_nrm[i].normalize()
            } else {
                Vec3::Y
            };
            let uv = sum_uv[i] / c;

            new_vertices[i].position = pos.to_array();
            new_vertices[i].normal = nrm.to_array();
            new_vertices[i].uv = uv.to_array();
        }

        // Ремап индексов + выброс вырожденных треугольников.
        let mut new_indices: Vec<u32> = Vec::with_capacity(base_indices.len());
        for tri in base_indices.chunks_exact(3) {
            let a = remap_index(tri[0], base_vertices, &cluster_map, min, inv_size, res);
            let b = remap_index(tri[1], base_vertices, &cluster_map, min, inv_size, res);
            let c = remap_index(tri[2], base_vertices, &cluster_map, min, inv_size, res);
            if a == b || b == c || a == c {
                continue; // вырожденный
            }
            new_indices.push(a);
            new_indices.push(b);
            new_indices.push(c);
        }

        let tri_count = new_indices.len() / 3;
        let ratio = tri_count as f32 / base_tri_count.max(1) as f32;

        // Если LOD почти не уменьшил меш — не добавляем.
        if ratio > 0.95 && !result.is_empty() {
            continue;
        }

        result.push(LodLevel {
            ratio,
            resolution: res,
            vertices: new_vertices,
            indices: new_indices,
        });
    }

    result
}

fn remap_index(
    orig: u32,
    base_vertices: &[Vertex3D],
    cluster_map: &HashMap<u32, u32>,
    min: Vec3,
    inv_size: f32,
    res: u32,
) -> u32 {
    let v = &base_vertices[orig as usize];
    let p = Vec3::from(v.position);
    let norm = (p - min) * inv_size;
    let cx = ((norm.x * res as f32) as u32).min(res - 1);
    let cy = ((norm.y * res as f32) as u32).min(res - 1);
    let cz = ((norm.z * res as f32) as u32).min(res - 1);
    let key = cx | (cy << 10) | (cz << 20);
    *cluster_map.get(&key).unwrap_or(&0)
}