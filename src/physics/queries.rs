//! Physics queries: raycast, overlap, sweeps.
//!
//! Работают по компонентам `Collider` + `Transform` + `RigidBody`,
//! а не по рендер-мешам. Это «игровая» геометрия: стены, двери, NPC,
//! хитбоксы — всё, что должно быть невидимым для игрока, но осязаемым.
//!
//! Отличие от `editor::picking`:
//!   * picking — по треугольникам меша, для клика мышью в редакторе;
//!   * physics queries — по коллайдерам, для геймплея.

use glam::Vec3;

use crate::ecs::{Entity, World};
use crate::game::components::{Parent, Transform, Visible};
use crate::game::world_matrix;

use super::components::{BodyType, Collider, RigidBody};

// ============================================================
// Фильтр
// ============================================================

#[derive(Debug, Clone, Copy)]
pub struct QueryFilter {
    pub include_static: bool,
    pub include_dynamic: bool,
    pub include_kinematic: bool,
    /// Игнорировать ли entity с `Visible(false)`.
    /// По умолчанию true — скрытые объекты не должны ловить лучи.
    pub visible_only: bool,
}

impl Default for QueryFilter {
    fn default() -> Self {
        Self {
            include_static: true,
            include_dynamic: true,
            include_kinematic: true,
            visible_only: true,
        }
    }
}

impl QueryFilter {
    pub fn all() -> Self { Self::default() }

    pub fn static_only() -> Self {
        Self { include_dynamic: false, include_kinematic: false, ..Self::default() }
    }

    pub fn dynamic_only() -> Self {
        Self { include_static: false, include_kinematic: false, ..Self::default() }
    }

    pub fn including_hidden(mut self) -> Self {
        self.visible_only = false;
        self
    }

    fn accepts(&self, bt: BodyType) -> bool {
        match bt {
            BodyType::Static => self.include_static,
            BodyType::Dynamic => self.include_dynamic,
            BodyType::Kinematic => self.include_kinematic,
        }
    }
}

// ============================================================
// Результаты
// ============================================================

#[derive(Debug, Clone, Copy)]
pub struct RayHit {
    pub entity: Entity,
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
}

// ============================================================
// Снапшот коллайдеров (мировые позиции, с учётом Parent)
// ============================================================

struct ColliderSnapshot {
    entity: Entity,
    collider: Collider,
    /// Мировая позиция центра.
    position: Vec3,
    /// Мировой масштаб (abs).
    scale: Vec3,
    /// Мировой поворот. Для AABB и Capsule пока не применяется
    /// (см. примечание в `raycast_collider`), но сохраняется,
    /// чтобы future-proof'ить API.
    #[allow(dead_code)]
    rotation: glam::Quat,
}

fn collect(world: &World, filter: &QueryFilter) -> Vec<ColliderSnapshot> {
    let mut out = Vec::new();
    for &e in world.entities() {
        if filter.visible_only {
            if let Some(v) = world.get::<Visible>(e) {
                if !v.0 { continue; }
            }
        }
        let Some(col) = world.get::<Collider>(e).copied() else { continue };
        let Some(t) = world.get::<Transform>(e) else { continue };

        let bt = world.get::<RigidBody>(e).map(|rb| rb.body_type).unwrap_or(BodyType::Static);
        if !filter.accepts(bt) { continue; }

        let (position, rotation, scale) = if world.has::<Parent>(e) {
            let model = world_matrix(world, e);
            let (s, r, p) = model.to_scale_rotation_translation();
            (p, r, s.abs())
        } else {
            (t.position, t.rotation, t.scale.abs())
        };

        out.push(ColliderSnapshot { entity: e, collider: col, position, scale, rotation });
    }
    out
}

// ============================================================
// Публичный API
// ============================================================

impl super::PhysicsWorld {
    /// Ближайшее попадание луча. `dir` нормализуется внутри.
    pub fn raycast(
        &self,
        world: &World,
        origin: Vec3,
        dir: Vec3,
        max_dist: f32,
        filter: QueryFilter,
    ) -> Option<RayHit> {
        let dir = dir.normalize_or_zero();
        if dir.length_squared() < 1e-12 { return None; }
        let mut best: Option<RayHit> = None;
        for s in collect(world, &filter) {
            let Some((t, n)) = ray_collider(origin, dir, &s) else { continue };
            if t > max_dist { continue; }
            if best.map_or(true, |b| t < b.distance) {
                best = Some(RayHit {
                    entity: s.entity,
                    point: origin + dir * t,
                    normal: n,
                    distance: t,
                });
            }
        }
        best
    }

    /// Все попадания, отсортированные по дистанции.
    /// Полезно для penetration-оружия, лазеров, ricochet.
    pub fn raycast_all(
        &self,
        world: &World,
        origin: Vec3,
        dir: Vec3,
        max_dist: f32,
        filter: QueryFilter,
    ) -> Vec<RayHit> {
        let dir = dir.normalize_or_zero();
        if dir.length_squared() < 1e-12 { return Vec::new(); }
        let mut hits = Vec::new();
        for s in collect(world, &filter) {
            let Some((t, n)) = ray_collider(origin, dir, &s) else { continue };
            if t > max_dist { continue; }
            hits.push(RayHit { entity: s.entity, point: origin + dir * t, normal: n, distance: t });
        }
        hits.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
        hits
    }

    /// Есть ли хоть что-то на пути луча. Быстрее `raycast`, потому
    /// что не строит `RayHit` — полезно для line-of-sight.
    pub fn ray_blocked(
        &self,
        world: &World,
        origin: Vec3,
        dir: Vec3,
        max_dist: f32,
        filter: QueryFilter,
    ) -> bool {
        let dir = dir.normalize_or_zero();
        if dir.length_squared() < 1e-12 { return false; }
        for s in collect(world, &filter) {
            if let Some((t, _)) = ray_collider(origin, dir, &s) {
                if t <= max_dist { return true; }
            }
        }
        false
    }

    /// Все entity, чей коллайдер пересекается со сферой.
    pub fn overlap_sphere(
        &self,
        world: &World,
        center: Vec3,
        radius: f32,
        filter: QueryFilter,
    ) -> Vec<Entity> {
        let mut out = Vec::new();
        let s_min = center - Vec3::splat(radius);
        let s_max = center + Vec3::splat(radius);
        for s in collect(world, &filter) {
            let (bmin, bmax) = world_aabb(&s);
            if !aabb_overlap(s_min, s_max, bmin, bmax) { continue; }
            if sphere_hits(center, radius, &s) { out.push(s.entity); }
        }
        out
    }

    /// Все entity, чей коллайдер пересекается с AABB.
    pub fn overlap_aabb(
        &self,
        world: &World,
        min: Vec3,
        max: Vec3,
        filter: QueryFilter,
    ) -> Vec<Entity> {
        let mut out = Vec::new();
        for s in collect(world, &filter) {
            let (bmin, bmax) = world_aabb(&s);
            if aabb_overlap(min, max, bmin, bmax) { out.push(s.entity); }
        }
        out
    }

    /// Все entity в капсуле (сегмент `a`–`b` + радиус).
    pub fn overlap_capsule(
        &self,
        world: &World,
        a: Vec3,
        b: Vec3,
        radius: f32,
        filter: QueryFilter,
    ) -> Vec<Entity> {
        let mut out = Vec::new();
        let c = (a + b) * 0.5;
        let half = (b - a) * 0.5;
        let cap_min = c - half.abs() - Vec3::splat(radius);
        let cap_max = c + half.abs() + Vec3::splat(radius);
        for s in collect(world, &filter) {
            let (bmin, bmax) = world_aabb(&s);
            if !aabb_overlap(cap_min, cap_max, bmin, bmax) { continue; }
            if capsule_hits(a, b, radius, &s) { out.push(s.entity); }
        }
        out
    }
}

// ============================================================
// Ray vs collider
// ============================================================

fn ray_collider(origin: Vec3, dir: Vec3, s: &ColliderSnapshot) -> Option<(f32, Vec3)> {
    match s.collider {
        Collider::Sphere { radius } => {
            ray_sphere(origin, dir, s.position, radius * s.scale.max_element())
        }
        Collider::Aabb { half_extents } => {
            let h = half_extents * s.scale;
            ray_aabb(origin, dir, s.position - h, s.position + h)
        }
        Collider::Capsule { radius, height } => {
            // ВАЖНО: капсула трактуется как вертикальная мировая.
            // Это согласуется с `physics::world::global_aabb` и
            // `collision::collider_world_aabb` — там тоже так.
            // Когда добавим rotation dynamics (Фаза 10), нужно будет
            // трансформировать луч в локальное пространство коллайдера.
            let r = radius * s.scale.max_element();
            let hy = height * s.scale.y * 0.5;
            ray_capsule(
                origin, dir,
                s.position - Vec3::Y * hy,
                s.position + Vec3::Y * hy,
                r,
            )
        }
    }
}

fn ray_sphere(origin: Vec3, dir: Vec3, center: Vec3, radius: f32) -> Option<(f32, Vec3)> {
    let oc = origin - center;
    let b = oc.dot(dir);
    let c = oc.length_squared() - radius * radius;
    let disc = b * b - c;
    if disc < 0.0 { return None; }
    let sq = disc.sqrt();
    let t1 = -b - sq;
    let t2 = -b + sq;
    let t = if t1 > 1e-6 { t1 } else if t2 > 1e-6 { t2 } else { return None };
    let point = origin + dir * t;
    let n = (point - center).normalize_or_zero();
    Some((t, n))
}

fn ray_aabb(origin: Vec3, dir: Vec3, bmin: Vec3, bmax: Vec3) -> Option<(f32, Vec3)> {
    let mut tmin = 0.0f32;
    let mut tmax = f32::INFINITY;
    let mut normal = Vec3::ZERO;
    for i in 0..3 {
        let o = origin[i];
        let d = dir[i];
        if d.abs() < 1e-8 {
            if o < bmin[i] || o > bmax[i] { return None; }
        } else {
            let inv = 1.0 / d;
            let mut t1 = (bmin[i] - o) * inv;
            let mut t2 = (bmax[i] - o) * inv;
            let sign = if inv < 0.0 { -1.0 } else { 1.0 };
            if t1 > t2 { std::mem::swap(&mut t1, &mut t2); }
            if t1 > tmin {
                tmin = t1;
                let mut n = Vec3::ZERO;
                n[i] = sign;
                normal = n;
            }
            if t2 < tmax { tmax = t2; }
            if tmin > tmax { return None; }
        }
    }
    if tmin <= 1e-6 {
        // Origin внутри AABB — «попали сразу».
        return Some((0.0, -dir));
    }
    Some((tmin, normal))
}

/// Ray-vs-capsule (Iraklis 2012). Возвращает (t, normal) ближайшего
/// положительного пересечения.
fn ray_capsule(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, radius: f32) -> Option<(f32, Vec3)> {
    let ba = b - a;
    let oa = origin - a;
    let baba = ba.dot(ba);
    let bard = ba.dot(dir);
    let baoa = ba.dot(oa);
    let rdoa = dir.dot(oa);
    let oaoa = oa.dot(oa);
    let a_coef = baba - bard * bard;
    let b_coef = baba * rdoa - baoa * bard;
    let c_coef = baba * oaoa - baoa * baoa - radius * radius * baba;

    // Тело цилиндра.
    if a_coef.abs() > 1e-12 {
        let h = b_coef * b_coef - a_coef * c_coef;
        if h >= 0.0 {
            let t = (-b_coef - h.sqrt()) / a_coef;
            if t > 1e-6 {
                let y = baoa + t * bard;
                if y > 0.0 && y < baba {
                    let point = origin + dir * t;
                    let seg = a + ba * (y / baba);
                    let n = (point - seg).normalize_or_zero();
                    return Some((t, n));
                }
            }
        }
    }

    // Сферические крышки — берём ближайшее из двух.
    let mut best: Option<(f32, Vec3)> = None;
    for (center, sgn) in [(a, -1.0f32), (b, 1.0)] {
        if let Some((t, n)) = ray_sphere(origin, dir, center, radius) {
            // Отсекаем «внутренние» попадания: крышка валидна,
            // только если точка за соответствующим концом.
            let pt = origin + dir * t;
            let along = (pt - a).dot(ba) / baba.max(1e-12);
            let on_cap = if sgn < 0.0 { along <= 0.0 } else { along >= 1.0 };
            if !on_cap { continue; }
            if best.map_or(true, |(bt, _)| t < bt) {
                best = Some((t, n));
            }
        }
    }
    best
}

// ============================================================
// Overlap helpers
// ============================================================

fn world_aabb(s: &ColliderSnapshot) -> (Vec3, Vec3) {
    match s.collider {
        Collider::Sphere { radius } => {
            let r = radius * s.scale.max_element();
            (s.position - Vec3::splat(r), s.position + Vec3::splat(r))
        }
        Collider::Aabb { half_extents } => {
            let h = half_extents * s.scale;
            (s.position - h, s.position + h)
        }
        Collider::Capsule { radius, height } => {
            let r = radius * s.scale.max_element();
            let hy = height * s.scale.y * 0.5;
            let h = Vec3::new(r, hy, r);
            (s.position - h, s.position + h)
        }
    }
}

fn sphere_hits(center: Vec3, radius: f32, s: &ColliderSnapshot) -> bool {
    match s.collider {
        Collider::Sphere { radius: r } => {
            let rr = r * s.scale.max_element();
            (center - s.position).length_squared() < (radius + rr) * (radius + rr)
        }
        Collider::Aabb { half_extents } => {
            let h = half_extents * s.scale;
            let p = center.clamp(s.position - h, s.position + h);
            (center - p).length_squared() < radius * radius
        }
        Collider::Capsule { radius: r, height } => {
            let rr = r * s.scale.max_element();
            let hy = height * s.scale.y * 0.5;
            let a = s.position - Vec3::Y * hy;
            let b = s.position + Vec3::Y * hy;
            let p = closest_on_segment(center, a, b);
            (center - p).length_squared() < (radius + rr) * (radius + rr)
        }
    }
}

fn capsule_hits(a: Vec3, b: Vec3, radius: f32, s: &ColliderSnapshot) -> bool {
    // Segment-vs-collider — через проверку нескольких точек вдоль
    // сегмента + радиус. Для геймплея этого достаточно; при росте
    // сцены можно перейти на GJK.
    let samples = 8;
    for i in 0..=samples {
        let t = i as f32 / samples as f32;
        let p = a.lerp(b, t);
        if sphere_hits(p, radius, s) { return true; }
    }
    false
}

fn closest_on_segment(p: Vec3, a: Vec3, b: Vec3) -> Vec3 {
    let ab = b - a;
    let t = (p - a).dot(ab) / ab.length_squared().max(1e-12);
    a + ab * t.clamp(0.0, 1.0)
}

#[inline]
fn aabb_overlap(amin: Vec3, amax: Vec3, bmin: Vec3, bmax: Vec3) -> bool {
    amin.x <= bmax.x && amax.x >= bmin.x
        && amin.y <= bmax.y && amax.y >= bmin.y
        && amin.z <= bmax.z && amax.z >= bmin.z
}