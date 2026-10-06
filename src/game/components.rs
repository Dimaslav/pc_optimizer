use glam::{Mat4, Quat, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { position: Vec3::new(x, y, z), rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
    pub fn at(position: Vec3) -> Self {
        Self { position, rotation: Quat::IDENTITY, scale: Vec3::ONE }
    }
    pub fn with_scale(mut self, s: f32) -> Self { self.scale = Vec3::splat(s); self }
    pub fn with_scale_xyz(mut self, x: f32, y: f32, z: f32) -> Self { self.scale = Vec3::new(x, y, z); self }
    pub fn with_rotation(mut self, q: Quat) -> Self { self.rotation = q; self }
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Parent(pub u32);

#[derive(Debug, Clone)]
pub struct Name(pub String);
impl Name { pub fn new(s: impl Into<String>) -> Self { Self(s.into()) } }

#[derive(Debug, Clone)]
pub struct MeshHandle(pub String);

#[derive(Debug, Clone)]
pub struct MaterialHandle(pub String);

#[derive(Debug, Clone)]
pub struct SkeletonHandle(pub String);

#[derive(Debug, Clone, Copy)]
pub struct Tint(pub [f32; 4]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Visible(pub bool);

#[derive(Debug, Clone, Copy)]
pub struct TextureTiling {
    pub size: f32,
}

impl Default for TextureTiling {
    fn default() -> Self { Self { size: 1.0 } }
}

impl TextureTiling {
    pub fn new(size: f32) -> Self {
        Self { size: size.max(0.001) }
    }
}

#[derive(Debug, Clone)]
pub struct AnimationPlayer {
    pub clip: String,
    pub time: f32,
    pub speed: f32,
    pub looping: bool,
}
impl AnimationPlayer {
    pub fn new(clip: impl Into<String>) -> Self {
        Self { clip: clip.into(), time: 0.0, speed: 1.0, looping: true }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Spinner {
    pub axis: Vec3,
    pub speed: f32,
}
impl Spinner { pub fn new(axis: Vec3, speed: f32) -> Self { Self { axis, speed } } }

#[derive(Debug, Clone, Copy)]
pub struct Velocity {
    pub value: Vec3,
}
impl Velocity {
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { value: Vec3::new(x, y, z) } }
}

// ============================================================
// Play-режим
// ============================================================

#[derive(Debug, Clone, Copy)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}
impl Health {
    pub fn new(max: f32) -> Self { Self { current: max, max } }
}

#[derive(Debug, Clone, Copy)]
pub struct Chase {
    pub speed: f32,
    pub stop_distance: f32,
}
impl Chase {
    pub fn new(speed: f32, stop_distance: f32) -> Self {
        Self { speed, stop_distance }
    }
}

#[derive(Debug, Clone)]
pub struct Trigger {
    pub radius: f32,
    pub action: TriggerAction,
    pub once: bool,
    pub fired: bool,
}

#[derive(Debug, Clone)]
pub enum TriggerAction {
    Teleport([f32; 3]),
    Tint([f32; 4]),
    Despawn,
    CallElevator { elevator: u32, floor_idx: u32 },
    PlaySound(String),
}

impl Trigger {
    pub fn new(radius: f32, action: TriggerAction) -> Self {
        Self { radius, action, once: true, fired: false }
    }
    pub fn repeatable(radius: f32, action: TriggerAction) -> Self {
        Self { radius, action, once: false, fired: false }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Interactable {
    Pickup,
    Paint([f32; 4]),
    Toggle,
}

// ============================================================
// Лифт
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevatorState {
    Idle,
    Moving,
    DoorsOpening,
    DoorsOpen,
    DoorsClosing,
}

impl Default for ElevatorState {
    fn default() -> Self { Self::Idle }
}

/// Логика лифта с trapezoid-профилем скорости и sensor-дверью.
///
/// **КРИТИЧНО:** сама платформа двигается через `RigidBody::kinematic()`.
/// `ElevatorSystem` пишет `velocity.y` kinematic-тела, а `physics.step`
/// интегрирует позицию. Прямое изменение `Transform.position` сломает
/// динамику: dynamic-объекты (ящики) на платформе не поедут.
#[derive(Debug, Clone)]
pub struct Elevator {
    /// Y-координаты этажей (обычно 2+).
    pub floors: Vec<f32>,
    pub current_floor: usize,
    pub target_floor: usize,

    /// Максимальная скорость (м/с).
    pub speed: f32,
    /// Ускорение (м/с²). Отвечает за плавный разгон/торможение.
    /// 0.01 — мгновенный старт; 5.0 — «плавно как лифт в отеле».
    pub acceleration: f32,
    /// Текущая линейная скорость по Y со знаком. Пишется в
    /// `RigidBody.velocity.y` kinematic-тела каждый кадр.
    pub current_velocity: f32,

    pub state: ElevatorState,
    pub doors_open: f32,
    pub door_speed: f32,
    pub dwell: f32,
    pub dwell_timer: f32,

    /// Радиус XZ сенсора двери. Игрок внутри — двери не закрываются.
    /// Должен быть немного больше ширины кабины, чтобы ловить
    /// стоящих в проёме.
    pub sensor_radius: f32,
    /// true — игрок в sensor-зоне в этом кадре (обновляется App).
    pub player_inside: bool,
}

impl Elevator {
    pub fn new(floors: Vec<f32>, speed: f32) -> Self {
        let f = if floors.len() < 2 { vec![0.0, 3.0] } else { floors };
        Self {
            floors: f,
            current_floor: 0,
            target_floor: 0,
            speed: speed.max(0.1),
            acceleration: 3.0,
            current_velocity: 0.0,
            state: ElevatorState::Idle,
            doors_open: 0.0,
            door_speed: 1.5,
            dwell: 2.5,
            dwell_timer: 0.0,
            sensor_radius: 2.0,
            player_inside: false,
        }
    }

    pub fn floor_y(&self, idx: usize) -> f32 {
        self.floors.get(idx).copied().unwrap_or(0.0)
    }

    pub fn current_y(&self) -> f32 {
        self.floor_y(self.current_floor)
    }

    /// Вызвать лифт на этаж `floor_idx`.
    pub fn call(&mut self, floor_idx: usize) {
        if floor_idx >= self.floors.len() { return; }

        if self.state == ElevatorState::Moving {
            self.target_floor = floor_idx;
            return;
        }

        if floor_idx == self.current_floor {
            if matches!(self.state, ElevatorState::Idle | ElevatorState::DoorsClosing) {
                self.state = ElevatorState::DoorsOpening;
            }
            return;
        }

        self.target_floor = floor_idx;
        if self.doors_open > 0.01 {
            self.state = ElevatorState::DoorsClosing;
        } else {
            self.state = ElevatorState::Moving;
        }
    }

    /// Обновление состояния `Moving` для одного кадра.
    /// Возвращает `true`, если приехали на этаж.
    ///
    /// `current_y` — текущая Y платформы (из Transform).
    /// Логика: trapezoid (разгон → круиз → торможение).
    pub fn update_moving(&mut self, current_y: f32, dt: f32) -> bool {
        let target_y = self.floor_y(self.target_floor);
        let diff = target_y - current_y;
        let dist = diff.abs();

        // Если можем доехать за один кадр — финализируем точно.
        // Иначе из-за интегрирования позиции в physics.step
        // перескочим на пару сантиметров выше/ниже этажа.
        let finish_threshold = (self.current_velocity.abs() * dt * 2.0).max(0.02);
        if dist <= finish_threshold {
            // velocity такой, чтобы за один шаг физики попасть ровно.
            self.current_velocity = diff / dt.max(1e-4);
            self.current_floor = self.target_floor;
            return true;
        }

        let sign = diff.signum();
        let a = self.acceleration.max(0.01);

        // Скорость торможения: v = sqrt(2 * a * distance).
        // С ней комфортно остановиться ровно на этаже.
        let braking_speed = (2.0 * a * dist).sqrt();
        let desired_speed = self.speed.min(braking_speed);
        let desired_velocity = sign * desired_speed;

        // Плавно двигаемся к целевой скорости с ускорением `a`.
        let dv = desired_velocity - self.current_velocity;
        let max_dv = a * dt;
        if dv.abs() <= max_dv {
            self.current_velocity = desired_velocity;
        } else {
            self.current_velocity += dv.signum() * max_dv;
        }

        false
    }

    /// Общая частота вызовов в секунду для FSM.
    /// Если приехали — velocity обнуляется при завершении.
    pub fn stop_velocity(&mut self) {
        self.current_velocity = 0.0;
    }
}

/// Дверь-створка. Двигается по `slide_axis` от `closed_position`.
///
/// Обычно привязана к платформе лифта через `Parent`. `closed_position`
/// и `slide_axis` задаются в **локальных координатах** родителя.
#[derive(Debug, Clone, Copy)]
pub struct SlidingDoor {
    pub open_amount: f32,
    pub target: f32,
    pub speed: f32,
    pub slide_axis: Vec3,
    pub slide_distance: f32,
    pub closed_position: Vec3,
}

impl SlidingDoor {
    pub fn new(closed_position: Vec3, slide_axis: Vec3, slide_distance: f32) -> Self {
        Self {
            open_amount: 0.0,
            target: 0.0,
            speed: 1.2,
            slide_axis: slide_axis.normalize_or_zero(),
            slide_distance: slide_distance.max(0.01),
            closed_position,
        }
    }
}