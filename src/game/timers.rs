//! Таймеры для геймплея: кулдауны, задержки, периодические события.
//!
//! `Timer` — компонент. `TimerSystem` — тикает их каждый кадр.
//!
//! # Применение
//!
//! ```ignore
//! // Одноразовая задержка (например, спавн через 2 секунды):
//! world.insert(e, Timer::new(2.0));
//!
//! // Кулдаун стрельбы:
//! if world.get::<Timer>(player).map_or(true, |t| t.is_finished()) {
//!     // стреляем
//!     if let Some(t) = world.get_mut::<Timer>(player) { t.restart(); }
//! }
//!
//! // Периодический спавн:
//! world.insert(spawner, Timer::repeating(3.0));
//! for (_, t) in world.query::<Timer>() {
//!     if t.just_finished() { /* новый враг */ }
//! }
//!
//! // Подписка на события (надёжнее, чем polling):
//! for ev in world.read_events::<TimerFinished>() {
//!     if ev.entity == my_entity { /* ... */ }
//! }
//! ```
//!
//! # Порядок систем
//!
//! `TimerSystem` должен идти **первым** в списке систем кадра:
//! `just_finished()` становится `true` в момент `tick()` и остаётся
//! таким до следующего `tick()`. Если `TimerSystem` отработал первым,
//! все остальные системы в этом кадре увидят `just_finished() == true`
//! у завершившихся таймеров.
//!
//! # Отличие от `engine::time`
//!
//! `engine::time` — глобальное время кадра (FPS, dt, frame_count).
//! `Timer` — per-entity таймер, живёт в ECS, тикается в игровом
//! времени (`dt` клампится `Time::MAX_DELTA`).

use crate::ecs::{Entity, System, World};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerMode {
    /// Срабатывает один раз и останавливается.
    Once,
    /// Срабатывает каждые `duration` секунд, пока не удалят.
    Repeating,
}

impl Default for TimerMode {
    fn default() -> Self {
        Self::Once
    }
}

/// Per-entity таймер.
///
/// Живёт в ECS, продвигается `TimerSystem`. Полностью безопасен для
/// `Send + Sync + 'static` — все поля примитивные.
#[derive(Debug, Clone)]
pub struct Timer {
    pub duration: f32,
    pub elapsed: f32,
    pub mode: TimerMode,
    pub paused: bool,
    /// Установлен в `true` в кадре завершения. Сбрасывается в начале
    /// следующего `tick`. Даёт возможность polling-проверки.
    just_finished: bool,
    /// `true` после завершения Once-таймера. У Repeating всегда `false`.
    finished: bool,
}

impl Timer {
    /// Одноразовый таймер.
    pub fn new(duration: f32) -> Self {
        Self::with_mode(duration, TimerMode::Once)
    }

    /// Периодический таймер.
    pub fn repeating(duration: f32) -> Self {
        Self::with_mode(duration, TimerMode::Repeating)
    }

    pub fn with_mode(duration: f32, mode: TimerMode) -> Self {
        let d = duration.max(1e-6);
        Self {
            duration: d,
            elapsed: 0.0,
            mode,
            paused: false,
            just_finished: false,
            finished: false,
        }
    }

    /// Таймер, который уже завершён. Удобно для инициализации
    /// кулдауна в состоянии «готов стрелять».
    pub fn finished_now(duration: f32) -> Self {
        let mut t = Self::new(duration);
        t.elapsed = t.duration;
        t.finished = true;
        t
    }

    pub fn tick(&mut self, dt: f32) {
        // Сбрасываем флаг «только что завершился» в начале каждого tick.
        self.just_finished = false;

        if self.paused { return; }
        if matches!(self.mode, TimerMode::Once) && self.finished { return; }
        if dt <= 0.0 { return; }

        self.elapsed += dt;
        if self.elapsed < self.duration { return; }

        match self.mode {
            TimerMode::Once => {
                self.elapsed = self.duration;
                self.finished = true;
                self.just_finished = true;
            }
            TimerMode::Repeating => {
                // При большом dt таймер может «перескочить» несколько
                // периодов. Оставляем только дробную часть, а
                // `just_finished` ставим один раз — для геймплея
                // важно «был ли период в этом кадре», а не «сколько».
                self.elapsed %= self.duration;
                self.just_finished = true;
            }
        }
    }

    /// Начать заново (с нуля).
    pub fn restart(&mut self) {
        self.reset();
    }

    pub fn reset(&mut self) {
        self.elapsed = 0.0;
        self.finished = false;
        self.just_finished = false;
    }

    pub fn pause(&mut self) { self.paused = true; }
    pub fn resume(&mut self) { self.paused = false; }
    pub fn toggle_pause(&mut self) { self.paused = !self.paused; }

    pub fn set_duration(&mut self, d: f32) {
        self.duration = d.max(1e-6);
        if self.elapsed > self.duration {
            self.elapsed = self.duration;
        }
    }

    /// Прогресс в диапазоне [0, 1]. Удобно для UI-полосок и lerp'ов.
    pub fn fraction(&self) -> f32 {
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    /// Остаток до завершения.
    pub fn remaining(&self) -> f32 {
        (self.duration - self.elapsed).max(0.0)
    }

    pub fn is_finished(&self) -> bool { self.finished }
    pub fn is_paused(&self) -> bool { self.paused }
    pub fn just_finished(&self) -> bool { self.just_finished }
}

impl Default for Timer {
    fn default() -> Self {
        Self::new(1.0)
    }
}

/// Событие: таймер завершился в этом кадре.
///
/// Отправляется `TimerSystem` **один раз** за кадр на каждую entity
/// с завершившимся `Timer`. Для Repeating — каждый период, для Once —
/// ровно один раз.
///
/// Приходит и как polling (`Timer::just_finished`), и как событие —
/// выбирай что удобнее для конкретной системы.
#[derive(Debug, Clone, Copy)]
pub struct TimerFinished {
    pub entity: Entity,
}

/// Система, продвигающая все `Timer` в мире.
///
/// **Ставь первой** в списке систем кадра.
pub struct TimerSystem;

impl System for TimerSystem {
    fn update(&mut self, world: &mut World, dt: f32) {
        // Фаза 1: тикаем и собираем завершившиеся.
        // Не отправляем события прямо в цикле — `query_mut` держит
        // mutable borrow `World`, а `world.send` нужен тоже mutable.
        let mut just_finished: Vec<Entity> = Vec::new();

        for (e, t) in world.query_mut::<Timer>() {
            t.tick(dt);
            if t.just_finished() {
                just_finished.push(e);
            }
        }

        // Фаза 2: рассылаем события. `Vec::new()` в типичном кадре
        // не аллоцирует — аллокация только при первом завершении.
        for e in just_finished {
            world.send(TimerFinished { entity: e });
        }
    }
}