use bytemuck::{Pod, Zeroable};
use glam::Vec3;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct LineVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
}

impl LineVertex {
    const ATTRS: [wgpu::VertexAttribute; 2] =
        wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &Self::ATTRS,
        }
    }
}

pub struct LineBatch {
    vertices: Vec<LineVertex>,
}

impl LineBatch {
    pub fn new() -> Self {
        Self { vertices: Vec::new() }
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
    }

    pub fn line(&mut self, a: Vec3, b: Vec3, color: [f32; 4]) {
        self.vertices.push(LineVertex {
            position: a.to_array(),
            color,
        });
        self.vertices.push(LineVertex {
            position: b.to_array(),
            color,
        });
    }

    pub fn vertices(&self) -> &[LineVertex] {
        &self.vertices
    }

    pub fn len(&self) -> usize {
        self.vertices.len() / 2
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }

    pub fn grid(
        &mut self,
        half_extent: f32,
        step: f32,
        color_minor: [f32; 4],
        color_major: [f32; 4],
        major_every: u32,
    ) {
        let n = (half_extent / step).round() as i32;
        for i in -n..=n {
            let t = i as f32 * step;
            let major = i % (major_every as i32) == 0;
            let c = if major { color_major } else { color_minor };

            self.line(
                Vec3::new(-half_extent, 0.0, t),
                Vec3::new(half_extent, 0.0, t),
                c,
            );
            self.line(
                Vec3::new(t, 0.0, -half_extent),
                Vec3::new(t, 0.0, half_extent),
                c,
            );
        }
    }

    pub fn axes(&mut self, length: f32) {
        self.line(
            Vec3::ZERO,
            Vec3::new(length, 0.0, 0.0),
            [1.0, 0.15, 0.15, 1.0],
        );
        self.line(
            Vec3::ZERO,
            Vec3::new(0.0, length, 0.0),
            [0.15, 1.0, 0.15, 1.0],
        );
        self.line(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, length),
            [0.15, 0.4, 1.0, 1.0],
        );
    }

    pub fn box_wireframe(&mut self, min: Vec3, max: Vec3, color: [f32; 4]) {
        let c = [
            Vec3::new(min.x, min.y, min.z),
            Vec3::new(max.x, min.y, min.z),
            Vec3::new(max.x, max.y, min.z),
            Vec3::new(min.x, max.y, min.z),
            Vec3::new(min.x, min.y, max.z),
            Vec3::new(max.x, min.y, max.z),
            Vec3::new(max.x, max.y, max.z),
            Vec3::new(min.x, max.y, max.z),
        ];
        let edges = [
            (0, 1), (1, 2), (2, 3), (3, 0),
            (4, 5), (5, 6), (6, 7), (7, 4),
            (0, 4), (1, 5), (2, 6), (3, 7),
        ];
        for (a, b) in edges {
            self.line(c[a], c[b], color);
        }
    }

    /// Wireframe-сфера: три ортогональных круга.
    pub fn sphere_wireframe(
        &mut self,
        center: Vec3,
        radius: f32,
        color: [f32; 4],
        segments: u32,
    ) {
        let n = segments.max(8);
        for i in 0..n {
            let a1 = i as f32 / n as f32 * std::f32::consts::TAU;
            let a2 = (i + 1) as f32 / n as f32 * std::f32::consts::TAU;

            let (c1, s1) = a1.sin_cos();
            let (c2, s2) = a2.sin_cos();

            // XZ
            self.line(
                center + Vec3::new(c1 * radius, 0.0, s1 * radius),
                center + Vec3::new(c2 * radius, 0.0, s2 * radius),
                color,
            );
            // XY
            self.line(
                center + Vec3::new(c1 * radius, s1 * radius, 0.0),
                center + Vec3::new(c2 * radius, s2 * radius, 0.0),
                color,
            );
            // YZ
            self.line(
                center + Vec3::new(0.0, c1 * radius, s1 * radius),
                center + Vec3::new(0.0, c2 * radius, s2 * radius),
                color,
            );
        }
    }
}

impl Default for LineBatch {
    fn default() -> Self {
        Self::new()
    }
}

pub struct LineBuffer {
    pub buffer: wgpu::Buffer,
    pub capacity: u64,
    pub vertex_count: u32,
}

impl LineBuffer {
    pub fn new(device: &wgpu::Device, initial_capacity: u64) -> Self {
        let capacity = initial_capacity.max(1);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("line_buffer"),
            size: capacity * std::mem::size_of::<LineVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            buffer,
            capacity,
            vertex_count: 0,
        }
    }

    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[LineVertex]) {
        self.vertex_count = vertices.len() as u32;
        if vertices.is_empty() {
            return;
        }

        if vertices.len() as u64 > self.capacity {
            let new_cap = (self.capacity * 2).max(vertices.len() as u64);
            self.buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("line_buffer"),
                size: new_cap * std::mem::size_of::<LineVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.capacity = new_cap;
        }

        queue.write_buffer(&self.buffer, 0, bytemuck::cast_slice(vertices));
    }
}