use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ============================================================
// Alpha / sampler types
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlphaMode {
    Opaque,
    Mask,
    Blend,
}

impl Default for AlphaMode {
    fn default() -> Self { Self::Opaque }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WrapMode { Repeat, ClampToEdge, MirroredRepeat }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SamplerFilter { Nearest, Linear }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SamplerDesc {
    pub wrap_u: WrapMode,
    pub wrap_v: WrapMode,
    pub mag_filter: SamplerFilter,
    pub min_filter: SamplerFilter,
    pub mip_filter: SamplerFilter,
}

impl Default for SamplerDesc {
    fn default() -> Self {
        Self {
            wrap_u: WrapMode::Repeat,
            wrap_v: WrapMode::Repeat,
            mag_filter: SamplerFilter::Linear,
            min_filter: SamplerFilter::Linear,
            mip_filter: SamplerFilter::Linear,
        }
    }
}

impl SamplerDesc {
    pub fn to_wgpu(&self) -> wgpu::SamplerDescriptor<'static> {
        let wrap = |w: WrapMode| match w {
            WrapMode::Repeat         => wgpu::AddressMode::Repeat,
            WrapMode::ClampToEdge    => wgpu::AddressMode::ClampToEdge,
            WrapMode::MirroredRepeat => wgpu::AddressMode::MirrorRepeat,
        };
        let flt = |f: SamplerFilter| match f {
            SamplerFilter::Nearest => wgpu::FilterMode::Nearest,
            SamplerFilter::Linear  => wgpu::FilterMode::Linear,
        };
        wgpu::SamplerDescriptor {
            label: Some("material_sampler"),
            address_mode_u: wrap(self.wrap_u),
            address_mode_v: wrap(self.wrap_v),
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: flt(self.mag_filter),
            min_filter: flt(self.min_filter),
            mipmap_filter: flt(self.mip_filter),
            ..Default::default()
        }
    }
}

// ============================================================
// Material
// ============================================================

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub base_color: [f32; 4],
    pub base_color_texture: Option<String>,

    pub metallic: f32,
    pub roughness: f32,
    pub metallic_roughness_texture: Option<String>,

    pub normal_texture: Option<String>,
    pub normal_scale: f32,

    pub emissive: [f32; 3],
    pub emissive_texture: Option<String>,

    pub alpha_mode: AlphaMode,
    pub alpha_cutoff: f32,
    pub double_sided: bool,
    pub sampler: SamplerDesc,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [1.0, 1.0, 1.0, 1.0],
            base_color_texture: None,
            metallic: 0.0,
            roughness: 0.8,
            metallic_roughness_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            emissive: [0.0, 0.0, 0.0],
            emissive_texture: None,
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            double_sided: false,
            sampler: SamplerDesc::default(),
        }
    }
}

impl Material {
    pub fn new(base_color: [f32; 4]) -> Self {
        Self { base_color, ..Default::default() }
    }

    pub fn with_texture(mut self, name: impl Into<String>) -> Self {
        self.base_color_texture = Some(name.into()); self
    }

    pub fn with_metallic_roughness(mut self, metallic: f32, roughness: f32) -> Self {
        self.metallic = metallic; self.roughness = roughness; self
    }

    pub fn with_mr_texture(mut self, name: impl Into<String>) -> Self {
        self.metallic_roughness_texture = Some(name.into()); self
    }

    pub fn with_normal_texture(mut self, name: impl Into<String>) -> Self {
        self.normal_texture = Some(name.into()); self
    }

    pub fn with_normal_scale(mut self, scale: f32) -> Self {
        self.normal_scale = scale; self
    }

    pub fn with_emissive(mut self, rgb: [f32; 3]) -> Self {
        self.emissive = rgb; self
    }

    pub fn with_emissive_texture(mut self, name: impl Into<String>) -> Self {
        self.emissive_texture = Some(name.into()); self
    }

    pub fn with_alpha_mode(mut self, mode: AlphaMode) -> Self {
        self.alpha_mode = mode; self
    }
    pub fn with_alpha_cutoff(mut self, c: f32) -> Self {
        self.alpha_cutoff = c; self
    }
    pub fn with_double_sided(mut self, v: bool) -> Self {
        self.double_sided = v; self
    }
    pub fn with_sampler(mut self, s: SamplerDesc) -> Self {
        self.sampler = s; self
    }

    pub fn is_blend(&self) -> bool {
        self.alpha_mode == AlphaMode::Blend
    }

    /// Имена текстур, на которые ссылается материал.
    /// Используется при сериализации сцены — чтобы собрать только
    /// релевантные текстуры.
    pub fn referenced_textures(&self) -> Vec<&str> {
        let mut out = Vec::with_capacity(4);
        if let Some(n) = &self.base_color_texture { out.push(n.as_str()); }
        if let Some(n) = &self.metallic_roughness_texture { out.push(n.as_str()); }
        if let Some(n) = &self.normal_texture { out.push(n.as_str()); }
        if let Some(n) = &self.emissive_texture { out.push(n.as_str()); }
        out
    }
}

// ============================================================
// Registry
// ============================================================

#[derive(Default)]
pub struct MaterialRegistry {
    materials: HashMap<String, Material>,
}

impl MaterialRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn insert(&mut self, name: impl Into<String>, mat: Material) {
        self.materials.insert(name.into(), mat);
    }

    pub fn get(&self, name: &str) -> Option<&Material> {
        self.materials.get(name)
    }

    pub fn get_or_default<'a>(&'a self, name: &str, default: &'a Material) -> &'a Material {
        self.materials.get(name).unwrap_or(default)
    }

    /// Отсортированный список имён материалов.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.materials.keys().cloned().collect();
        v.sort();
        v
    }

    /// Итератор по всем парам (имя, материал).
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Material)> {
        self.materials.iter()
    }
}