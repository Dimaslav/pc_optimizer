use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};
use std::sync::OnceLock;
use wgpu::util::DeviceExt;

use crate::render::bvh::Bvh;
use crate::render::lod::{generate_lods, LodLevel};

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex3D {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub joints: [u32; 4],
    pub weights: [f32; 4],
}

impl Vertex3D {
    pub fn static_vertex(
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        color: [f32; 4],
    ) -> Self {
        Self {
            position,
            normal,
            uv,
            color,
            joints: [0; 4],
            weights: [0.0; 4],
        }
    }

    const ATTRS: [wgpu::VertexAttribute; 6] = wgpu::vertex_attr_array![
        0 => Float32x3,
        1 => Float32x3,
        2 => Float32x2,
        3 => Float32x4,
        4 => Uint32x4,
        5 => Float32x4
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct InstanceData {
    pub model: [[f32; 4]; 4],
    pub normal_matrix: [[f32; 4]; 4],
    pub color: [f32; 4],
    /// `xy` — множитель UV (сколько раз текстура повторяется),
    /// `zw` — padding для 16-байтового выравнивания.
    pub uv_scale: [f32; 4],
    /// Матрица модели из **предыдущего кадра**. Нужна G-buffer'у для
    /// per-object motion vectors: prev_world_pos = prev_model * local_pos.
    /// Для статических объектов == model.
    pub prev_model: [[f32; 4]; 4],
}

impl InstanceData {
    /// Устаревший конструктор без motion: prev_model = model (статика).
    pub fn new(model: Mat4, color: [f32; 4]) -> Self {
        Self::new_with_uv(model, color, [1.0, 1.0])
    }

    /// То же, с UV-масштабом. prev_model = model → только camera motion.
    pub fn new_with_uv(model: Mat4, color: [f32; 4], uv_scale: [f32; 2]) -> Self {
        Self::new_full(model, model, color, uv_scale)
    }

    /// Полный конструктор: явный prev_model для per-object motion vectors.
    pub fn new_full(
        model: Mat4,
        prev_model: Mat4,
        color: [f32; 4],
        uv_scale: [f32; 2],
    ) -> Self {
        let m3 = Mat3::from_mat4(model);
        let normal = if m3.determinant().abs() > 1e-8 {
            m3.inverse().transpose()
        } else {
            Mat3::IDENTITY
        };
        Self {
            model: model.to_cols_array_2d(),
            normal_matrix: Mat4::from_mat3(normal).to_cols_array_2d(),
            color,
            uv_scale: [uv_scale[0], uv_scale[1], 0.0, 0.0],
            prev_model: prev_model.to_cols_array_2d(),
        }
    }

    const ATTRS: [wgpu::VertexAttribute; 14] = wgpu::vertex_attr_array![
        6  => Float32x4, 7  => Float32x4, 8  => Float32x4, 9  => Float32x4,
        10 => Float32x4, 11 => Float32x4, 12 => Float32x4, 13 => Float32x4,
        14 => Float32x4,
        15 => Float32x4,
        16 => Float32x4, 17 => Float32x4, 18 => Float32x4, 19 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRS,
        }
    }
}

pub struct Mesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
    pub bounds_center: Vec3,
    pub bounds_radius: f32,
    pub aabb_min: Vec3,
    pub aabb_max: Vec3,
    pub triangles: Vec<[Vec3; 3]>,

    /// BVH для raycast — строится **лениво** при первом обращении
    /// через `bvh()`. Раньше строился eager в `Mesh::new`, что
    /// заметно замедляло загрузку больших FBX/glTF моделей, хотя
    /// BVH нужен только для picking / projectile-хиттеста.
    bvh: OnceLock<Bvh>,

    // CPU-копии для экспорта (FBX и т.п.).
    pub cpu_vertices: Vec<Vertex3D>,
    pub cpu_indices: Vec<u32>,

    /// Автоматически сгенерированные LOD-уровни (не считая LOD0).
    /// Пустой, если меш слишком простой.
    pub lods: Vec<LodLevel>,
}

impl Mesh {
    /// Ленивое построение BVH. При первом вызове строит из
    /// `self.triangles`, дальше возвращает закэшированное значение.
    ///
    /// `OnceLock` потокобезопасен и не требует `&mut self`, поэтому
    /// можно звать из `&Mesh` в picking'е и в projectile-цикле.
    pub fn bvh(&self) -> &Bvh {
        self.bvh.get_or_init(|| Bvh::build(&self.triangles))
    }

    /// Публичный конструктор: строит меш + генерирует LOD.
    pub fn new(
        device: &wgpu::Device,
        vertices: &[Vertex3D],
        indices: &[u32],
        label: &str,
    ) -> Self {
        Self::from_raw_parts(device, vertices, indices, label, true)
    }

    /// Конструктор без генерации LOD. Для внутреннего использования
    /// (`Renderer::add_mesh` при регистрации LOD-версий).
    pub fn from_raw_parts(
        device: &wgpu::Device,
        vertices: &[Vertex3D],
        indices: &[u32],
        label: &str,
        generate_lods_too: bool,
    ) -> Self {
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label}_vb")),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{label}_ib")),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for v in vertices {
            let p = Vec3::from(v.position);
            min = min.min(p);
            max = max.max(p);
        }
        let center = (min + max) * 0.5;
        let mut radius = 0.0f32;
        for v in vertices {
            let p = Vec3::from(v.position);
            radius = radius.max((p - center).length());
        }
        let aabb_min = min;
        let aabb_max = max;

        let mut triangles = Vec::with_capacity(indices.len() / 3);
        for tri in indices.chunks_exact(3) {
            let a = Vec3::from(vertices[tri[0] as usize].position);
            let b = Vec3::from(vertices[tri[1] as usize].position);
            let c = Vec3::from(vertices[tri[2] as usize].position);
            triangles.push([a, b, c]);
        }

        let lods = if generate_lods_too && triangles.len() >= 200 {
            generate_lods(vertices, indices, 3)
        } else {
            Vec::new()
        };

        Self {
            vertex_buffer,
            index_buffer,
            index_count: indices.len() as u32,
            bounds_center: center,
            bounds_radius: radius,
            aabb_min,
            aabb_max,
            triangles,
            bvh: OnceLock::new(),
            cpu_vertices: vertices.to_vec(),
            cpu_indices: indices.to_vec(),
            lods,
        }
    }

    pub fn world_bounds(&self, model: &Mat4) -> (Vec3, f32) {
        let center_world = model.transform_point3(self.bounds_center);
        let m3 = Mat3::from_mat4(*model);
        let sx = m3.x_axis.length();
        let sy = m3.y_axis.length();
        let sz = m3.z_axis.length();
        let scale = sx.max(sy).max(sz);
        (center_world, self.bounds_radius * scale)
    }

    pub fn world_aabb(&self, model: &Mat4) -> (Vec3, Vec3) {
        let mn = self.aabb_min;
        let mx = self.aabb_max;

        let corners = [
            Vec3::new(mn.x, mn.y, mn.z),
            Vec3::new(mx.x, mn.y, mn.z),
            Vec3::new(mn.x, mx.y, mn.z),
            Vec3::new(mx.x, mx.y, mn.z),
            Vec3::new(mn.x, mn.y, mx.z),
            Vec3::new(mx.x, mn.y, mx.z),
            Vec3::new(mn.x, mx.y, mx.z),
            Vec3::new(mx.x, mx.y, mx.z),
        ];

        let mut wmin = Vec3::splat(f32::INFINITY);
        let mut wmax = Vec3::splat(f32::NEG_INFINITY);
        for c in corners {
            let w = model.transform_point3(c);
            wmin = wmin.min(w);
            wmax = wmax.max(w);
        }
        (wmin, wmax)
    }

    // ============================================================
    // Генераторы
    // ============================================================

    pub fn cube(device: &wgpu::Device, size: f32) -> Self {
        let h = size * 0.5;
        let mut vertices = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);

        let faces: [([f32; 3], [f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([0.0, 0.0, 1.0],  [1.0, 0.0, 0.0],  [0.0, 1.0, 0.0],  [0.0, 0.0, 1.0]),
            ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0],  [0.0, 0.0, -1.0]),
            ([1.0, 0.0, 0.0],  [0.0, 0.0, -1.0], [0.0, 1.0, 0.0],  [1.0, 0.0, 0.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0],  [0.0, 1.0, 0.0],  [-1.0, 0.0, 0.0]),
            ([0.0, 1.0, 0.0],  [1.0, 0.0, 0.0],  [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0],  [0.0, 0.0, 1.0],  [0.0, -1.0, 0.0]),
        ];

        for (normal, right, up, base) in faces.iter() {
            let start = vertices.len() as u32;
            let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
            let uvs = [(0.0f32, 1.0f32), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)];

            for (i, (cr, cu)) in corners.iter().enumerate() {
                let pos = [
                    (base[0] + right[0] * cr + up[0] * cu) * h,
                    (base[1] + right[1] * cr + up[1] * cu) * h,
                    (base[2] + right[2] * cr + up[2] * cu) * h,
                ];
                vertices.push(Vertex3D::static_vertex(
                    pos, *normal, [uvs[i].0, uvs[i].1], [1.0; 4],
                ));
            }
            indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
        }

        Self::new(device, &vertices, &indices, "cube")
    }

    pub fn sphere(device: &wgpu::Device, radius: f32, rings: u32, segments: u32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for ring in 0..=rings {
            let phi = std::f32::consts::PI * ring as f32 / rings as f32;
            let (sp, cp) = phi.sin_cos();
            let y = cp * radius;
            let r = sp * radius;

            for seg in 0..=segments {
                let theta = std::f32::consts::TAU * seg as f32 / segments as f32;
                let (st, ct) = theta.sin_cos();
                vertices.push(Vertex3D::static_vertex(
                    [ct * r, y, st * r],
                    [ct * sp, cp, st * sp],
                    [seg as f32 / segments as f32, ring as f32 / rings as f32],
                    [1.0; 4],
                ));
            }
        }

        let stride = segments + 1;
        for ring in 0..rings {
            for seg in 0..segments {
                let a = ring * stride + seg;
                let b = a + stride;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }

        Self::new(device, &vertices, &indices, "sphere")
    }

    pub fn plane(device: &wgpu::Device, size: f32, subdivisions: u32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let n = subdivisions.max(1);
        let h = size * 0.5;

        for z in 0..=n {
            for x in 0..=n {
                let fx = x as f32 / n as f32;
                let fz = z as f32 / n as f32;
                vertices.push(Vertex3D::static_vertex(
                    [fx * size - h, 0.0, fz * size - h],
                    [0.0, 1.0, 0.0],
                    [fx, fz],
                    [1.0; 4],
                ));
            }
        }

        let stride = n + 1;
        for z in 0..n {
            for x in 0..n {
                let a = z * stride + x;
                let b = a + stride;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }

        Self::new(device, &vertices, &indices, "plane")
    }

    pub fn plane_xy(device: &wgpu::Device, size: f32, subdivisions: u32) -> Self {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let n = subdivisions.max(1);
        let h = size * 0.5;

        for y in 0..=n {
            for x in 0..=n {
                let fx = x as f32 / n as f32;
                let fy = y as f32 / n as f32;
                vertices.push(Vertex3D::static_vertex(
                    [fx * size - h, fy * size - h, 0.0],
                    [0.0, 0.0, 1.0],
                    [fx, 1.0 - fy],
                    [1.0; 4],
                ));
            }
        }

        let stride = n + 1;
        for y in 0..n {
            for x in 0..n {
                let a = y * stride + x;
                let b = a + stride;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }

        Self::new(device, &vertices, &indices, "plane_xy")
    }

    pub fn truncated_cone(
        device: &wgpu::Device,
        r_bottom: f32,
        r_top: f32,
        height: f32,
        segments: u32,
    ) -> Self {
        let segs = segments.max(3);
        let h2 = height * 0.5;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        let slope = (r_bottom - r_top) / height.max(1e-4);
        for i in 0..=segs {
            let theta = std::f32::consts::TAU * i as f32 / segs as f32;
            let (st, ct) = theta.sin_cos();
            let n_raw = Vec3::new(ct, slope, st).normalize_or_zero();

            vertices.push(Vertex3D::static_vertex(
                [ct * r_bottom, -h2, st * r_bottom],
                n_raw.to_array(),
                [i as f32 / segs as f32, 0.0],
                [1.0; 4],
            ));
            vertices.push(Vertex3D::static_vertex(
                [ct * r_top, h2, st * r_top],
                n_raw.to_array(),
                [i as f32 / segs as f32, 1.0],
                [1.0; 4],
            ));
        }
        for i in 0..segs {
            let a = i * 2;
            let b = a + 1;
            let c = a + 2;
            let d = a + 3;
            indices.extend_from_slice(&[a, b, c, c, b, d]);
        }

        if r_bottom > 1e-6 {
            let center = vertices.len() as u32;
            vertices.push(Vertex3D::static_vertex(
                [0.0, -h2, 0.0], [0.0, -1.0, 0.0], [0.5, 0.5], [1.0; 4],
            ));
            let start = vertices.len() as u32;
            for i in 0..=segs {
                let theta = std::f32::consts::TAU * i as f32 / segs as f32;
                let (st, ct) = theta.sin_cos();
                vertices.push(Vertex3D::static_vertex(
                    [ct * r_bottom, -h2, st * r_bottom],
                    [0.0, -1.0, 0.0],
                    [ct * 0.5 + 0.5, st * 0.5 + 0.5],
                    [1.0; 4],
                ));
            }
            for i in 0..segs {
                indices.extend_from_slice(&[center, start + i, start + i + 1]);
            }
        }

        if r_top > 1e-6 {
            let center = vertices.len() as u32;
            vertices.push(Vertex3D::static_vertex(
                [0.0, h2, 0.0], [0.0, 1.0, 0.0], [0.5, 0.5], [1.0; 4],
            ));
            let start = vertices.len() as u32;
            for i in 0..=segs {
                let theta = std::f32::consts::TAU * i as f32 / segs as f32;
                let (st, ct) = theta.sin_cos();
                vertices.push(Vertex3D::static_vertex(
                    [ct * r_top, h2, st * r_top],
                    [0.0, 1.0, 0.0],
                    [ct * 0.5 + 0.5, st * 0.5 + 0.5],
                    [1.0; 4],
                ));
            }
            for i in 0..segs {
                indices.extend_from_slice(&[center, start + i + 1, start + i]);
            }
        }

        Self::new(device, &vertices, &indices, "cylinder")
    }

    pub fn cylinder(device: &wgpu::Device, radius: f32, height: f32, segments: u32) -> Self {
        Self::truncated_cone(device, radius, radius, height, segments)
    }

    pub fn cone(device: &wgpu::Device, radius: f32, height: f32, segments: u32) -> Self {
        Self::truncated_cone(device, radius, 0.0, height, segments)
    }

    pub fn capsule(
        device: &wgpu::Device,
        radius: f32,
        cylinder_height: f32,
        hemi_rings: u32,
        segments: u32,
    ) -> Self {
        let segs = segments.max(3);
        let hr = hemi_rings.max(2);
        let h2 = cylinder_height * 0.5;
        let total_rings = 2 * hr + 1;

        let mut vertices = Vec::new();
        let mut indices = Vec::new();

        for ring in 0..=total_rings {
            let (y, r_axis) = if ring <= hr {
                let phi = std::f32::consts::PI * 0.5 * ring as f32 / hr as f32;
                let (sp, cp) = phi.sin_cos();
                (h2 + cp * radius, sp * radius)
            } else {
                let phi = std::f32::consts::PI * 0.5 * (total_rings - ring) as f32 / hr as f32;
                let (sp, cp) = phi.sin_cos();
                (-h2 - cp * radius, sp * radius)
            };

            for s in 0..=segs {
                let theta = std::f32::consts::TAU * s as f32 / segs as f32;
                let (st, ct) = theta.sin_cos();
                let center_y = if ring <= hr { h2 } else { -h2 };
                let n = Vec3::new(ct * r_axis, y - center_y, st * r_axis).normalize_or_zero();
                vertices.push(Vertex3D::static_vertex(
                    [ct * r_axis, y, st * r_axis],
                    n.to_array(),
                    [s as f32 / segs as f32, ring as f32 / total_rings as f32],
                    [1.0; 4],
                ));
            }
        }

        let stride = segs + 1;
        for ring in 0..total_rings {
            for s in 0..segs {
                let a = ring * stride + s;
                let b = a + stride;
                indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
            }
        }

        Self::new(device, &vertices, &indices, "capsule")
    }
}