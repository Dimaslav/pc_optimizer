//! Коллизии игрока со сценой.
//!
//! Использует компонент `Collider` (Sphere/Aabb/Capsule) вместо AABB
//! меша. Это даёт корректные коллизии для стен, дверей, NPC и любых
//! объектов, у которых меш не совпадает с логикой.
//!
//! **Dynamic-тела не блокируют игрока по XZ** — он проходит сквозь их
//! footprint, а `character::push_dynamic_bodies` после этого толкает
//! пересекающиеся тела. По Y dynamic-тела блокируют — можно стоять
//! на ящике.

use glam::Vec3;

use crate::ecs::{Entity, World};
use crate::game::components::Visible;
use crate::physics::{BodyType, Collider, RigidBody};

const CAPSULE_SAMPLES: usize = 8;
/// Допуск по Y для определения «стоит на поверхности».
const SUPPORT_TOLERANCE: f32 = 0.15;

#[derive(Clone, Copy)]
pub struct PlayerCapsule {
    pub radius: f32,
    pub height: f32,
}

impl Default for PlayerCapsule {
    fn default() -> Self {
        Self { radius: 0.35, height: 1.8 }
    }
}

#[inline]
fn point_aabb_dist_sq(p: Vec3, min: Vec3, max: Vec3) -> f32 {
    let clamped = p.clamp(min, max);
    (p - clamped).length_squared()
}

#[inline]
fn capsule_hits_aabb(a: Vec3, b: Vec3, r: f32, amin: Vec3, amax: Vec3) -> bool {
    let r2 = r * r;
    let center = (amin + amax) * 0.5;
    let half = (amax - amin) * 0.5;
    let bs_radius = half.length();
    let capsule_center = (a + b) * 0.5;
    if (center - capsule_center).length() > bs_radius + r + (b - a).length() * 0.5 {
        return false;
    }
    for i in 0..=CAPSULE_SAMPLES {
        let t = i as f32 / CAPSULE_SAMPLES as f32;
        let p = a.lerp(b, t);
        if point_aabb_dist_sq(p, amin, amax) < r2 {
            return true;
        }
    }
    false
}

/// Мировой AABB для коллайдера сущности.
fn collider_world_aabb(col: &Collider, world_pos: Vec3, world_scale: Vec3) -> (Vec3, Vec3) {
    match col {
        Collider::Sphere { radius } => {
            let r = radius * world_scale.max_element();
            (world_pos - Vec3::splat(r), world_pos + Vec3::splat(r))
        }
        Collider::Aabb { half_extents } => {
            let h = *half_extents * world_scale;
            (world_pos - h, world_pos + h)
        }
        Collider::Capsule { radius, height } => {
            // ИСПРАВЛЕНО: `height` — полная высота капсулы (включая
            // обе полусферы-крышки), а не высота цилиндра.
            // См. doc-комментарий на `Collider::Capsule`.
            let r = radius * world_scale.max_element();
            let hy = height * world_scale.y * 0.5;
            let h = Vec3::new(r, hy, r);
            (world_pos - h, world_pos + h)
        }
    }
}

/// Мировая позиция и мировой масштаб сущности (с учётом Parent).
fn world_pos_scale(world: &World, e: Entity) -> Option<(Vec3, Vec3)> {
    let model = crate::game::world_matrix(world, e);
    let (s, _, t) = model.to_scale_rotation_translation();
    Some((t, s.abs()))
}

fn capsule_hits_impl(
    world: &World,
    feet_pos: Vec3,
    cap: &PlayerCapsule,
    include_dynamic: bool,
) -> bool {
    let r = cap.radius;
    let h = cap.height.max(2.0 * r + 1e-3);
    let a = feet_pos + Vec3::Y * r;
    let b = feet_pos + Vec3::Y * (h - r);

    for &e in world.entities() {
        if let Some(v) = world.get::<Visible>(e) {
            if !v.0 { continue; }
        }
        if !include_dynamic {
            if let Some(rb) = world.get::<RigidBody>(e) {
                if rb.body_type == BodyType::Dynamic { continue; }
            }
        }

        let Some(col) = world.get::<Collider>(e) else { continue };
        let Some((wp, ws)) = world_pos_scale(world, e) else { continue };

        let (amin, amax) = collider_world_aabb(col, wp, ws);
        if capsule_hits_aabb(a, b, r, amin, amax) {
            return true;
        }
    }
    false
}

fn capsule_hits(world: &World, feet_pos: Vec3, cap: &PlayerCapsule) -> bool {
    capsule_hits_impl(world, feet_pos, cap, false)
}

fn capsule_hits_with_dynamic(world: &World, feet_pos: Vec3, cap: &PlayerCapsule) -> bool {
    capsule_hits_impl(world, feet_pos, cap, true)
}

/// Ищет entity, на чьей верхней плоскости стоит игрок.
pub fn find_support_entity(
    world: &World,
    feet_pos: Vec3,
    cap: &PlayerCapsule,
) -> Option<Entity> {
    let r = cap.radius;
    let mut best: Option<(Entity, f32)> = None;

    for &e in world.entities() {
        if let Some(v) = world.get::<Visible>(e) {
            if !v.0 { continue; }
        }
        let Some(col) = world.get::<Collider>(e) else { continue };
        let Some((wp, ws)) = world_pos_scale(world, e) else { continue };

        let (amin, amax) = collider_world_aabb(col, wp, ws);
        let top_y = amax.y;
        let dy = feet_pos.y - top_y;
        if !(-0.02..=SUPPORT_TOLERANCE).contains(&dy) {
            continue;
        }
        if feet_pos.x + r < amin.x || feet_pos.x - r > amax.x {
            continue;
        }
        if feet_pos.z + r < amin.z || feet_pos.z - r > amax.z {
            continue;
        }

        if best.map_or(true, |(_, y)| top_y > y) {
            best = Some((e, top_y));
        }
    }
    best.map(|(e, _)| e)
}

/// Разрешает движение `delta` из `start_feet` с учётом коллизий.
///
/// Возвращает `(new_feet, on_ground, support_entity)`.
///
/// XZ — с игнорированием dynamic-тел (player passes through, потом толкает).
/// Y  — со всеми телами (можно стоять на ящике).
pub fn resolve_movement(
    world: &World,
    start_feet: Vec3,
    delta: Vec3,
    cap: &PlayerCapsule,
    floor_y: f32,
) -> (Vec3, bool, Option<Entity>) {
    let stuck = capsule_hits_with_dynamic(world, start_feet, cap);

    if stuck {
        let mut pos = start_feet + delta;
        if pos.y < floor_y {
            pos.y = floor_y;
        }
        let on_ground = (start_feet.y + delta.y) <= floor_y + 1e-4;
        let support = if on_ground {
            find_support_entity(world, pos, cap)
        } else {
            None
        };
        return (pos, on_ground, support);
    }

    let mut pos = start_feet;
    let mut on_ground = false;

    let can_step_up = start_feet.y <= floor_y + 1e-3;
    let step_heights = [0.15_f32, 0.30, 0.45];

    // === X — без dynamic ===
    if delta.x.abs() > 1e-6 {
        let try_pos = pos + Vec3::new(delta.x, 0.0, 0.0);
        if !capsule_hits(world, try_pos, cap) {
            pos = try_pos;
        } else if can_step_up {
            for h in step_heights {
                let lifted = try_pos + Vec3::new(0.0, h, 0.0);
                if !capsule_hits(world, lifted, cap) {
                    pos = lifted;
                    break;
                }
            }
        }
    }

    // === Z — без dynamic ===
    if delta.z.abs() > 1e-6 {
        let try_pos = pos + Vec3::new(0.0, 0.0, delta.z);
        if !capsule_hits(world, try_pos, cap) {
            pos = try_pos;
        } else if can_step_up {
            for h in step_heights {
                let lifted = try_pos + Vec3::new(0.0, h, 0.0);
                if !capsule_hits(world, lifted, cap) {
                    pos = lifted;
                    break;
                }
            }
        }
    }

    // === Y — субшагами, со всеми телами ===
    if delta.y.abs() > 1e-6 {
        let max_step = 0.1_f32;
        let steps = (delta.y.abs() / max_step).ceil().max(1.0) as i32;
        let step = delta.y / steps as f32;

        for _ in 0..steps {
            let try_pos = pos + Vec3::new(0.0, step, 0.0);
            if !capsule_hits_with_dynamic(world, try_pos, cap) {
                pos = try_pos;
            } else {
                if step < 0.0 {
                    on_ground = true;
                }
                break;
            }
        }
    }

    if pos.y <= floor_y {
        pos.y = floor_y;
        on_ground = true;
    }

    let support = find_support_entity(world, pos, cap);
    if support.is_some() {
        on_ground = true;
    }

    (pos, on_ground, support)
}