//! Палитра примитивов, источников света, decals и пресетов.

use glam::Vec3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteItem {
    Cube, Sphere, Cylinder, Cone, Capsule, Plane,
    Sun, PointLight,
    Decal,
    Enemy, Pickup, Switch, TriggerCube, Ground,
}

impl PaletteItem {
    pub fn all_primitives() -> &'static [PaletteItem] {
        &[
            PaletteItem::Cube,
            PaletteItem::Sphere,
            PaletteItem::Cylinder,
            PaletteItem::Cone,
            PaletteItem::Capsule,
            PaletteItem::Plane,
        ]
    }

    pub fn all_lights() -> &'static [PaletteItem] {
        &[PaletteItem::Sun, PaletteItem::PointLight]
    }

    pub fn all_decals() -> &'static [PaletteItem] {
        &[PaletteItem::Decal]
    }

    pub fn all_presets() -> &'static [PaletteItem] {
        &[
            PaletteItem::Enemy,
            PaletteItem::Pickup,
            PaletteItem::Switch,
            PaletteItem::TriggerCube,
            PaletteItem::Ground,
        ]
    }

    pub fn is_light(&self) -> bool {
        matches!(self, PaletteItem::Sun | PaletteItem::PointLight)
    }

    pub fn is_decal(&self) -> bool {
        matches!(self, PaletteItem::Decal)
    }

    pub fn label(&self) -> &'static str {
        match self {
            PaletteItem::Cube => "Cube",
            PaletteItem::Sphere => "Sphere",
            PaletteItem::Cylinder => "Cylinder",
            PaletteItem::Cone => "Cone",
            PaletteItem::Capsule => "Capsule",
            PaletteItem::Plane => "Plane",
            PaletteItem::Sun => "☀ Sun",
            PaletteItem::PointLight => "💡 Point",
            PaletteItem::Decal => "🎨 Decal",
            PaletteItem::Enemy => "Enemy",
            PaletteItem::Pickup => "Pickup",
            PaletteItem::Switch => "Switch",
            PaletteItem::TriggerCube => "Trigger",
            PaletteItem::Ground => "Ground",
        }
    }

    pub fn mesh(&self) -> &'static str {
        match self {
            PaletteItem::Cube => "cube",
            PaletteItem::Sphere => "sphere",
            PaletteItem::Cylinder => "cylinder",
            PaletteItem::Cone => "cone",
            PaletteItem::Capsule => "capsule",
            PaletteItem::Plane => "quad",
            PaletteItem::Decal => "cube",
            PaletteItem::Enemy => "sphere",
            PaletteItem::Pickup => "sphere",
            PaletteItem::Switch => "cube",
            PaletteItem::TriggerCube => "cube",
            PaletteItem::Ground => "ground",
            PaletteItem::Sun | PaletteItem::PointLight => {
                panic!("PaletteItem::mesh() called on light")
            }
        }
    }

    pub fn material(&self) -> &'static str {
        match self {
            PaletteItem::Cube => "flat_blue",
            PaletteItem::Sphere => "gold",
            PaletteItem::Cylinder => "flat_red",
            PaletteItem::Cone => "flat_red",
            PaletteItem::Capsule => "flat_blue",
            PaletteItem::Plane => "ground",
            PaletteItem::Decal => "flat_blue",
            PaletteItem::Enemy => "flat_red",
            PaletteItem::Pickup => "emissive",
            PaletteItem::Switch => "flat_blue",
            PaletteItem::TriggerCube => "glass",
            PaletteItem::Ground => "ground",
            PaletteItem::Sun | PaletteItem::PointLight => {
                panic!("PaletteItem::material() called on light")
            }
        }
    }

    pub fn half_height(&self) -> f32 {
        match self {
            PaletteItem::Cube => 0.5,
            PaletteItem::Sphere => 0.5,
            PaletteItem::Cylinder => 0.5,
            PaletteItem::Cone => 0.5,
            PaletteItem::Capsule => 0.75,
            PaletteItem::Plane => 0.0,
            PaletteItem::Sun => 10.0,
            PaletteItem::PointLight => 3.0,
            PaletteItem::Decal => 0.06,
            PaletteItem::Enemy => 0.4,
            PaletteItem::Pickup => 0.2,
            PaletteItem::Switch => 0.5,
            PaletteItem::TriggerCube => 0.5,
            PaletteItem::Ground => 0.0,
        }
    }

    pub fn default_scale(&self) -> f32 {
        match self {
            PaletteItem::Pickup => 0.4,
            PaletteItem::Enemy => 0.8,
            PaletteItem::Sun | PaletteItem::PointLight | PaletteItem::Decal => 1.0,
            _ => 1.0,
        }
    }

    pub fn default_tiling_size(&self) -> f32 {
        match self {
            PaletteItem::Ground => 8.0,
            PaletteItem::TriggerCube => 2.0,
            _ => 1.0,
        }
    }

    pub fn snaps_to_ground(&self) -> bool {
        !matches!(self, PaletteItem::Ground | PaletteItem::Decal)
    }
}

pub struct PaletteState {
    pub active: Option<PaletteItem>,
    pub preview_pos: Option<Vec3>,
    pub snap_to_grid: bool,
    pub grid_step: f32,
    pub keep_active: bool,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            active: None,
            preview_pos: None,
            snap_to_grid: true,
            grid_step: 0.5,
            keep_active: true,
        }
    }
}