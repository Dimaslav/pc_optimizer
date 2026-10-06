//! CPU particle system с burst-эмиттерами.
//!
//! Частицы — глобальный runtime-state в `App` (не ECS-сущности).
//! Каждый burst создаёт N частиц с начальной скоростью в конусе,
//! гравитацией и fade-out. Рендерятся крестами через `LineBatch`.

use glam::Vec3;

/// Максимум частиц в системе. Старые вытесняются новыми.
pub const MAX_PARTICLES: usize = 4096;

/// Состояние одной частицы.
#[derive(Clone, Copy)]
pub struct Particle {
    pub position: Vec3,
    pub velocity: Vec3,
    pub age: f32,
    pub lifetime: f32,
    pub size_start: f32,
    pub size_end: f32,
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    pub gravity: f32,
}

impl Particle {
    pub fn t(&self) -> f32 {
        (self.age / self.lifetime.max(1e-4)).clamp(0.0, 1.0)
    }

    pub fn color(&self) -> [f32; 4] {
        lerp4(self.color_start, self.color_end, self.t())
    }

    pub fn size(&self) -> f32 {
        self.size_start + (self.size_end - self.size_start) * self.t()
    }
}

/// Параметры одного burst-а.
#[derive(Clone, Copy)]
pub struct BurstParams {
    pub count: u32,
    pub lifetime: f32,
    /// Базовая скорость частицы.
    pub speed: f32,
    /// Угол конуса разлёта (радианы), от оси `dir`.
    pub spread: f32,
    /// Основное направление конуса. Обычно `Vec3::Y` или нормаль к поверхности.
    pub dir: Vec3,
    pub gravity: f32,
    pub size: f32,
    /// Конечный размер = size * size_end_scale.
    pub size_end_scale: f32,
    pub color_start: [f32; 4],
    pub color_end: [f32; 4],
    /// Разброс lifetime относительно базового: lifetime * (1 - var, 1 + var).
    pub lifetime_variation: f32,
}

impl Default for BurstParams {
    fn default() -> Self {
        Self {
            count: 32,
            lifetime: 0.6,
            speed: 4.0,
            spread: 0.6,
            dir: Vec3::Y,
            gravity: 9.0,
            size: 0.06,
            size_end_scale: 0.2,
            color_start: [1.0, 0.9, 0.5, 1.0],
            color_end: [1.0, 0.3, 0.1, 0.0],
            lifetime_variation: 0.3,
        }
    }
}

/// Создать burst из `params.count` частиц в позиции `origin`.
///
/// ИСПРАВЛЕНО: раньше при переполнении делался `out.remove(0)` — O(n)
/// сдвиг всего массива на каждую частицу. Теперь просто не эмитим
/// сверх лимита, а `Vec` резервируется заранее.
pub fn emit_burst(out: &mut Vec<Particle>, origin: Vec3, params: &BurstParams) {
    let dir = params.dir.normalize_or_zero();
    let dir = if dir.length_squared() < 1e-6 { Vec3::Y } else { dir };

    let free = MAX_PARTICLES.saturating_sub(out.len());
    if free == 0 { return; }
    let to_emit = (params.count as usize).min(free);

    if out.capacity() < out.len() + to_emit {
        out.reserve(to_emit);
    }

    for _ in 0..to_emit {
        let cos_spread = params.spread.cos();
        let cos_theta = 1.0 - rand01() * (1.0 - cos_spread);
        let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
        let phi = rand01() * std::f32::consts::TAU;

        let tangent = if dir.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
        let t1 = tangent.cross(dir).normalize_or_zero();
        let t2 = dir.cross(t1).normalize_or_zero();

        let local = Vec3::new(
            sin_theta * phi.cos(),
            sin_theta * phi.sin(),
            cos_theta,
        );
        let sample_dir = (t1 * local.x + t2 * local.y + dir * local.z).normalize_or_zero();

        let speed = params.speed * (0.7 + rand01() * 0.6);
        let lt = params.lifetime * (1.0 + (rand01() * 2.0 - 1.0) * params.lifetime_variation);

        out.push(Particle {
            position: origin,
            velocity: sample_dir * speed,
            age: 0.0,
            lifetime: lt.max(0.05),
            size_start: params.size,
            size_end: params.size * params.size_end_scale,
            color_start: params.color_start,
            color_end: params.color_end,
            gravity: params.gravity,
        });
    }
}

// ============================================================
// Готовые пресеты
// ============================================================

/// Искры от попадания пули в поверхность.
pub fn sparks(surface_normal: Vec3) -> BurstParams {
    BurstParams {
        count: 24,
        lifetime: 0.35,
        speed: 5.0,
        spread: 0.9,
        dir: surface_normal.normalize_or_zero(),
        gravity: 12.0,
        size: 0.05,
        size_end_scale: 0.2,
        color_start: [1.0, 0.95, 0.7, 1.0],
        color_end: [1.0, 0.4, 0.1, 0.0],
        lifetime_variation: 0.4,
    }
}

/// Взрыв врага / большой объект.
pub fn explosion() -> BurstParams {
    BurstParams {
        count: 64,
        lifetime: 1.0,
        speed: 7.0,
        spread: 1.5,
        dir: Vec3::Y,
        gravity: 6.0,
        size: 0.12,
        size_end_scale: 0.3,
        color_start: [1.0, 0.8, 0.3, 1.0],
        color_end: [0.6, 0.1, 0.05, 0.0],
        lifetime_variation: 0.35,
    }
}

/// Маленькая вспышка при подборе предмета.
pub fn pickup_glow() -> BurstParams {
    BurstParams {
        count: 32,
        lifetime: 0.7,
        speed: 2.5,
        spread: 2.5,
        dir: Vec3::Y,
        gravity: -1.5,
        size: 0.08,
        size_end_scale: 0.1,
        color_start: [1.0, 1.0, 0.5, 1.0],
        color_end: [0.9, 0.7, 1.0, 0.0],
        lifetime_variation: 0.3,
    }
}

/// Дым/пыль от попадания в землю.
pub fn dust_cloud() -> BurstParams {
    BurstParams {
        count: 40,
        lifetime: 1.2,
        speed: 2.0,
        spread: 2.0,
        dir: Vec3::Y,
        gravity: -0.5,
        size: 0.15,
        size_end_scale: 2.0,
        color_start: [0.7, 0.65, 0.6, 0.8],
        color_end: [0.5, 0.45, 0.4, 0.0],
        lifetime_variation: 0.4,
    }
}

// ============================================================
// Утилиты
// ============================================================

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// Простой быстрый рандом через thread_local LCG.
/// Достаточно для particles, не требует rand crate.
fn rand01() -> f32 {
    use std::cell::Cell;
    thread_local! {
        static SEED: Cell<u32> = const { Cell::new(0x1234_5678) };
    }
    SEED.with(|s| {
        let mut v = s.get();
        v = v.wrapping_mul(1664525).wrapping_add(1013904223);
        s.set(v);
        ((v >> 8) & 0xFFFFFF) as f32 / 16777215.0
    })
}