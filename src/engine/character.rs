//! Character controller: игрок толкает динамические тела.
//!
//! **Логика push:**
//!   1. Игрок движется через `collision::resolve_movement`, который
//!      игнорирует Dynamic-тела по XZ (см. `capsule_hits`). Так игрок
//!      не «упирается» в ящик, а проходит сквозь его footprint.
//!   2. После фактического перемещения `push_dynamic_bodies` находит
//!      dynamic-тела, пересекающиеся с капсулой игрока, и придаёт им
//!      velocity пропорционально скорости игрока.
//!   3. По Y dynamic-тела блокируют — игрок может стоять на ящике
//!      (`capsule_hits_with_dynamic`).
//!
//! **Ограничения v1:**
//!   - Игрок не получает обратный импульс (не отскакивает от тел).
//!   - Не учитывается rotation динамических тел.
//!   - Используется AABB approximation вместо точной капсулы.

use glam::Vec3;

use crate::ecs::{Entity, World};
use crate::game::components::Transform;
use crate::physics::{BodyType, Collider, RigidBody};

use super::collision::PlayerCapsule;

/// Толкает dynamic-тела, пересекающиеся с капсулой игрока.
///
/// Вызывается из `App::update_player` **после** `resolve_movement`,
/// с уже разрешённой позицией ног.
///
/// - `player_feet` — новая позиция ног.
/// - `player_delta` — фактическое смещение за кадр (`new_feet - old_feet`).
/// - `strength` — множитель. `1.0` = тело получает скорость,
///   равную скорости игрока; `0.5` = тело в 2 раза медленнее;
///   `0.0` = push отключён (проверка в вызывающем коде).
/// - `dt` — шаг времени (для перевода `delta` → velocity).
///
/// Контракт с телом: если тело движется медленнее `push_speed` в
/// направлении толчка — доускоряем до `push_speed`. Иначе — не трогаем
/// (чтобы не «замедлять» тело, которое летит быстрее игрока).
pub fn push_dynamic_bodies(
    world: &mut World,
    player_feet: Vec3,
    player_delta: Vec3,
    cap: &PlayerCapsule,
    dt: f32,
    strength: f32,
) {
    if dt <= 1e-6 || strength <= 0.0 {
        return;
    }

    // Толкаем только горизонтально: по Y push был бы игрушкой
    // (игрок не может «подбросить» ящик движением).
    let horizontal = Vec3::new(player_delta.x, 0.0, player_delta.z);
    if horizontal.length_squared() < 1e-8 {
        return;
    }
    let push_velocity = horizontal / dt * strength;
    let push_speed = push_velocity.length();
    if push_speed < 1e-3 {
        return;
    }
    let push_dir = push_velocity / push_speed;

    // AABB капсулы игрока (без rotation).
    let cap_min = Vec3::new(
        player_feet.x - cap.radius,
        player_feet.y,
        player_feet.z - cap.radius,
    );
    let cap_max = Vec3::new(
        player_feet.x + cap.radius,
        player_feet.y + cap.height,
        player_feet.z + cap.radius,
    );

    // Собираем кандидатов, чтобы не держать borrow World при мутации.
    let candidates: Vec<(Entity, Collider, Transform)> = world
        .entities()
        .iter()
        .copied()
        .filter_map(|e| {
            let rb = world.get::<RigidBody>(e)?;
            if rb.body_type != BodyType::Dynamic {
                return None;
            }
            let col = world.get::<Collider>(e)?;
            let t = world.get::<Transform>(e)?;
            Some((e, *col, *t))
        })
        .collect();

    for (e, col, t) in candidates {
        let (bmin, bmax) = collider_aabb(col, t.position, t.scale.abs());
        if !aabb_overlap(cap_min, cap_max, bmin, bmax) {
            continue;
        }

        // Тело пересекается с капсулой — толкаем.
        if let Some(rb) = world.get_mut::<RigidBody>(e) {
            let along = rb.velocity.dot(push_dir);
            if along < push_speed {
                rb.velocity += push_dir * (push_speed - along);
            }
            rb.wake();
        }
    }
}

/// Радиальный импульс взрыва.
///
/// Применяется ко всем dynamic-телам, чей центр находится в радиусе
/// `radius` от `center`. Величина импульса линейно падает до нуля
/// на границе (falloff).
///
/// В будущем можно вызывать из обработки попадания снаряда/гранаты.
pub fn apply_radial_impulse(
    world: &mut World,
    center: Vec3,
    radius: f32,
    power: f32,
) {
    if radius <= 0.0 || power <= 0.0 {
        return;
    }
    let r2 = radius * radius;

    let candidates: Vec<(Entity, Vec3)> = world
        .entities()
        .iter()
        .copied()
        .filter_map(|e| {
            let rb = world.get::<RigidBody>(e)?;
            if rb.body_type != BodyType::Dynamic {
                return None;
            }
            let t = world.get::<Transform>(e)?;
            let d2 = (t.position - center).length_squared();
            if d2 > r2 {
                return None;
            }
            Some((e, t.position))
        })
        .collect();

    for (e, pos) in candidates {
        let to_body = pos - center;
        let dist = to_body.length();
        if dist < 1e-6 {
            continue;
        }
        let dir = to_body / dist;
        let falloff = (1.0 - dist / radius).clamp(0.0, 1.0);
        let impulse = dir * power * falloff;

        if let Some(rb) = world.get_mut::<RigidBody>(e) {
            rb.apply_impulse(impulse);
        }
    }
}

// ============================================================
// Helpers
// ============================================================

fn collider_aabb(col: Collider, pos: Vec3, scale: Vec3) -> (Vec3, Vec3) {
    match col {
        Collider::Sphere { radius } => {
            let r = radius * scale.max_element();
            (pos - Vec3::splat(r), pos + Vec3::splat(r))
        }
        Collider::Aabb { half_extents } => {
            let h = half_extents * scale;
            (pos - h, pos + h)
        }
        Collider::Capsule { radius, height } => {
            // ИСПРАВЛЕНО: `height` — полная высота капсулы (включая
            // обе полусферы-крышки). См. `Collider::Capsule`.
            let r = radius * scale.max_element();
            let hy = height * scale.y * 0.5;
            let h = Vec3::new(r, hy, r);
            (pos - h, pos + h)
        }
    }
}

#[inline]
fn aabb_overlap(a_min: Vec3, a_max: Vec3, b_min: Vec3, b_max: Vec3) -> bool {
    a_min.x <= b_max.x
        && a_max.x >= b_min.x
        && a_min.y <= b_max.y
        && a_max.y >= b_min.y
        && a_min.z <= b_max.z
        && a_max.z >= b_min.z
}