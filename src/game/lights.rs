//! Источники света как ECS-компоненты.
//!
//! `DirectionalLight` и `PointLight` не связаны с рендером напрямую.
//! Игра собирает их из World каждый кадр и передаёт в Renderer через
//! `Game::dir_lights(world)` / `Game::point_lights(world)`.

use glam::Vec3;

/// Направленный свет (солнце, луна, прожектор без затухания).
///
/// `direction` — вектор ОТ сцены К источнику (то же соглашение, что
/// в шейдере: radiance = normalize(direction) * intensity).
/// `color` — RGB в линейном пространстве [0..1].
/// `intensity` — множитель HDR-яркости. Для солнца ~1.0–1.5.
#[derive(Debug, Clone, Copy)]
pub struct DirectionalLight {
    pub direction: Vec3,
    pub color: [f32; 3],
    pub intensity: f32,
}

impl DirectionalLight {
    /// Дневное солнце (используется по умолчанию).
    pub fn sun() -> Self {
        Self {
            direction: Vec3::new(0.4, 1.0, 0.3),
            color: [1.0, 0.98, 0.9],
            intensity: 1.2,
        }
    }

    /// Мягкий контровой свет — для заполнения теней.
    pub fn fill() -> Self {
        Self {
            direction: Vec3::new(-0.6, 0.3, -0.7),
            color: [1.0, 0.6, 0.4],
            intensity: 0.3,
        }
    }
}

impl Default for DirectionalLight {
    fn default() -> Self { Self::sun() }
}

/// Точечный свет с затуханием по квадрату расстояния до `range`.
///
/// `range` — радиус действия в метрах. Внутри диапазона — линейное
/// затухание `(1 - dist/range)²`, вне — свет не виден.
#[derive(Debug, Clone, Copy)]
pub struct PointLight {
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
}

impl PointLight {
    pub fn new(color: [f32; 3], intensity: f32, range: f32) -> Self {
        Self {
            color,
            intensity,
            range: range.max(0.1),
        }
    }
}

impl Default for PointLight {
    fn default() -> Self {
        Self {
            color: [1.0, 0.9, 0.7],
            intensity: 4.0,
            range: 15.0,
        }
    }
}