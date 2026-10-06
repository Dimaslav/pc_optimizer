//! GPU-структуры для decal-pass.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct DecalInstance {
    pub model: [[f32; 4]; 4],
    pub inv_model: [[f32; 4]; 4],
    pub tint: [f32; 4],
    pub _pad: [f32; 4],
}

impl DecalInstance {
    pub fn new(model: Mat4, tint: [f32; 4]) -> Self {
        let inv = model.inverse();
        Self {
            model: model.to_cols_array_2d(),
            inv_model: inv.to_cols_array_2d(),
            tint,
            _pad: [0.0; 4],
        }
    }

    // Locations 6..14 (9 vec4).
    const ATTRS: [wgpu::VertexAttribute; 9] = wgpu::vertex_attr_array![
        6  => Float32x4, 7  => Float32x4, 8  => Float32x4, 9  => Float32x4,
        10 => Float32x4, 11 => Float32x4, 12 => Float32x4, 13 => Float32x4,
        14 => Float32x4,
    ];

    pub fn layout() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as u64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &Self::ATTRS,
        }
    }
}

pub struct DecalDraw {
    pub texture: String,
    pub instances: Vec<DecalInstance>,
}