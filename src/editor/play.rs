//! Состояние Play-режима.

use crate::ecs::Entity;
use glam::Vec3;

pub struct PlayState {
    pub active: bool,

    /// ИЗМЕНЕНО (GameState): пауза внутри Play-режима.
    ///
    /// При `paused == true`:
    ///   * `Time::delta == 0.0` — физика, таймеры, анимации не тикают;
    ///   * `App::update_player` пропускается полностью (не обрабатываем
    ///     ввод движения, стрельбу, триггеры);
    ///   * рендер и UI продолжают работать, показывается overlay.
    ///
    /// `Escape` в Play → pause, `Escape` в Pause → resume.
    /// `F9` в Play или Pause → stop (выход из Play).
    pub paused: bool,

    pub saved_position: Vec3,
    pub vertical_velocity: f32,
    pub on_ground: bool,

    // === Движение ===
    pub eye_height: f32,
    pub player_radius: f32,
    pub player_height: f32,
    pub walk_speed: f32,
    pub run_speed: f32,
    pub jump_speed: f32,
    pub gravity: f32,
    pub floor_y: f32,
    pub look_sensitivity: f32,

    // === Crouch (приседание) ===
    pub crouch_height: f32,
    pub crouch_speed_mult: f32,
    pub current_eye_height: f32,
    pub crouching: bool,

    // === Character controller (Step 2) ===
    /// Множитель скорости, передаваемой dynamic-телам при контакте.
    /// 0.0 = push отключён, 1.0 = тела едут со скоростью игрока.
    pub push_strength: f32,

    // === Бой / HUD ===
    pub health: f32,
    pub max_health: f32,
    pub ammo: u32,
    pub max_ammo: u32,
    pub damage_per_shot: f32,
    pub gun_range: f32,
    pub bullet_speed: f32,
    pub fire_cooldown: f32,
    pub fire_cooldown_max: f32,
    pub interact_cooldown: f32,
    pub interact_cooldown_max: f32,
    pub interact_distance: f32,

    pub highlight: Option<Entity>,

    // === Head bob ===
    pub bob_enabled: bool,
    pub bob_amplitude: f32,
    pub bob_distance: f32,
    pub bob_current: f32,

    // === HUD ===
    pub show_crosshair: bool,
    pub show_hud: bool,
    pub show_health: bool,
    pub show_ammo: bool,
}

impl Default for PlayState {
    fn default() -> Self {
        Self {
            active: false,
            paused: false,
            saved_position: Vec3::new(0.0, 1.7, 45.0),
            vertical_velocity: 0.0,
            on_ground: true,

            eye_height: 1.7,
            player_radius: 0.35,
            player_height: 1.8,
            walk_speed: 6.0,
            run_speed: 12.0,
            jump_speed: 6.0,
            gravity: 20.0,
            floor_y: 0.0,
            look_sensitivity: 0.0025,

            crouch_height: 1.0,
            crouch_speed_mult: 0.5,
            current_eye_height: 1.7,
            crouching: false,

            push_strength: 1.0,

            health: 100.0,
            max_health: 100.0,
            ammo: 30,
            max_ammo: 30,
            damage_per_shot: 25.0,
            gun_range: 100.0,
            bullet_speed: 80.0,
            fire_cooldown: 0.0,
            fire_cooldown_max: 0.15,
            interact_cooldown: 0.0,
            interact_cooldown_max: 0.25,
            interact_distance: 5.0,
            highlight: None,

            bob_enabled: false,
            bob_amplitude: 0.03,
            bob_distance: 0.0,
            bob_current: 0.0,

            show_crosshair: true,
            show_hud: true,
            show_health: true,
            show_ammo: true,
        }
    }
}