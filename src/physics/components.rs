//! Компоненты физики: RigidBody, Collider, PhysicsMaterial.

use glam::Vec3;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BodyType {
    /// Не двигается. Бесконечная масса. Пол, стены.
    Static,
    /// Двигается под действием сил и гравитации.
    Dynamic,
    /// Двигается программно, не подчиняется силам.
    /// Для движущихся платформ, дверей, лифтов.
    Kinematic,
}

impl Default for BodyType {
    fn default() -> Self { Self::Dynamic }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct RigidBody {
    pub body_type: BodyType,
    /// Масса. Игнорируется для Static/Kinematic.
    pub mass: f32,

    pub velocity: Vec3,
    /// Пока не используется — задел под rotation dynamics.
    #[serde(default)]
    pub angular_velocity: Vec3,

    /// Экспоненциальное затухание: v *= (1 - damping·dt).
    pub linear_damping: f32,
    #[serde(default)]
    pub angular_damping: f32,

    /// Множитель гравитации (0 = невесомый, 1 = норма).
    pub gravity_scale: f32,

    /// Накопленная сила за кадр. Сбрасывается после `step`.
    #[serde(skip)]
    pub force: Vec3,
    /// Накопленный момент. Сбрасывается после `step`.
    #[serde(skip)]
    pub torque: Vec3,

    /// Тело спит — симуляция пропускается.
    #[serde(skip)]
    pub sleeping: bool,
    /// Сколько секунд подряд тело «тихое».
    #[serde(skip)]
    pub sleep_timer: f32,
}

impl RigidBody {
    pub fn dynamic(mass: f32) -> Self {
        Self {
            body_type: BodyType::Dynamic,
            mass: mass.max(1e-4),
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            linear_damping: 0.01,
            angular_damping: 0.05,
            gravity_scale: 1.0,
            force: Vec3::ZERO,
            torque: Vec3::ZERO,
            sleeping: false,
            sleep_timer: 0.0,
        }
    }

    pub fn static_body() -> Self {
        Self {
            body_type: BodyType::Static,
            mass: 0.0,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 0.0,
            force: Vec3::ZERO,
            torque: Vec3::ZERO,
            sleeping: false,
            sleep_timer: 0.0,
        }
    }

    pub fn kinematic() -> Self {
        Self {
            body_type: BodyType::Kinematic,
            mass: 0.0,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            linear_damping: 0.0,
            angular_damping: 0.0,
            gravity_scale: 0.0,
            force: Vec3::ZERO,
            torque: Vec3::ZERO,
            sleeping: false,
            sleep_timer: 0.0,
        }
    }

    pub fn inv_mass(&self) -> f32 {
        match self.body_type {
            BodyType::Dynamic if self.mass > 1e-8 => 1.0 / self.mass,
            _ => 0.0,
        }
    }

    pub fn is_dynamic(&self) -> bool {
        self.body_type == BodyType::Dynamic
    }

    pub fn is_static(&self) -> bool {
        self.body_type == BodyType::Static
    }

    /// Накопить силу (применится в следующем `step`).
    pub fn apply_force(&mut self, f: Vec3) {
        if self.is_dynamic() {
            self.force += f;
        }
    }

    /// Мгновенно изменить скорость (не зависит от шага физики).
    pub fn apply_impulse(&mut self, j: Vec3) {
        if self.is_dynamic() && self.mass > 1e-8 {
            self.velocity += j / self.mass;
            self.wake();
        }
    }

    pub fn wake(&mut self) {
        self.sleeping = false;
        self.sleep_timer = 0.0;
    }
}

impl Default for RigidBody {
    fn default() -> Self { Self::dynamic(1.0) }
}

/// Коллайдер. Центр совпадает с `Transform.position`.
///
/// Семантика `Capsule::height` — **полная высота, включая обе
/// полусферы-крышки**. То есть `capsule(0.4, 1.8)` — капсула,
/// у которой полная вертикальная протяжённость от нижней точки
/// до верхней равна 1.8 м, а радиус полусфер 0.4 м. Цилиндрическая
/// секция между полусферами получается `height - 2 * radius` (для
/// приведённого примера — 1.0 м).
///
/// Это соглашение согласовано с `engine::PlayerCapsule` (там
/// `height` тоже означает полную высоту). Раньше `Collider::Capsule`
/// интерпретировал `height` как **высоту цилиндра**, из-за чего
/// `capsule(0.4, 1.8)` давал коллайдер высотой 2.6 м при визуальной
/// высоте меша 1.6 м — персонаж «отталкивался» от NPC за 0.5 м до
/// касания.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Collider {
    /// Сфера, центр — в `Transform.position`.
    Sphere { radius: f32 },
    /// Axis-aligned box с центром в `Transform.position`.
    /// `half_extents` — в локальных единицах, масштабируются `Transform.scale`.
    /// При ненулевом повороте — используется внешний AABB вокруг повёрнутого бокса.
    Aabb { half_extents: Vec3 },
    /// Капсула вдоль локальной оси Y. `height` — полная высота,
    /// включая обе полусферы (см. doc-комментарий выше).
    Capsule { radius: f32, height: f32 },
}

impl Collider {
    pub fn sphere(radius: f32) -> Self {
        Self::Sphere { radius: radius.max(1e-4) }
    }

    pub fn aabb(half_extents: Vec3) -> Self {
        Self::Aabb { half_extents: half_extents.max(Vec3::splat(1e-4)) }
    }

    /// `height` — полная высота капсулы (включая обе полусферы).
    /// Значение автоматически поднимается до `2 * radius`, чтобы
    /// цилиндрическая секция не была отрицательной.
    pub fn capsule(radius: f32, height: f32) -> Self {
        let r = radius.max(1e-4);
        Self::Capsule {
            radius: r,
            height: height.max(2.0 * r + 1e-4),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PhysicsMaterial {
    /// Коэффициент восстановления: 0 = нет отскока, 1 = абсолютно упругий.
    pub restitution: f32,
    /// Коэффициент трения (Кулон): 0 = скольжение, >1 = сильно шероховатый.
    pub friction: f32,
}

impl PhysicsMaterial {
    pub fn new(restitution: f32, friction: f32) -> Self {
        Self {
            restitution: restitution.clamp(0.0, 1.0),
            friction: friction.max(0.0),
        }
    }

    pub fn rubber() -> Self { Self::new(0.7, 0.6) }
    pub fn metal() -> Self { Self::new(0.3, 0.4) }
    pub fn wood() -> Self { Self::new(0.2, 0.7) }
    pub fn ice() -> Self { Self::new(0.05, 0.05) }
    pub fn concrete() -> Self { Self::new(0.1, 0.9) }
}

impl Default for PhysicsMaterial {
    fn default() -> Self { Self::wood() }
}