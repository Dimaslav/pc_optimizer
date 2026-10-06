//! Анимационные события и рантайм анимаций.
//!
//! # Что это
//!
//! `AnimationEvent` — «маркер» на таймлайне анимационного клипа:
//! "в момент t секунд от начала клипа сработало событие name с
//! произвольным payload". Классические примеры: footstep, hit,
//! spawn_vfx, open_door, detach_weapon.
//!
//! # Как это работает
//!
//! События не хранятся в `AnimationClip` (glTF их не умеет), а живут
//! отдельно в `AnimationEvents` — реестре, который заполняется
//! программно или из sidecar-файла `.anim_events.ron`.
//!
//! На каждом кадре `AnimationRuntime::advance_all`:
//!   1. Продвигает `AnimationPlayer::time` для всех entity.
//!   2. Достаёт предыдущее значение `time` из side-table.
//!   3. Ищет события в интервале `(prev_time, new_time]` (с учётом
//!      wrap для looping-клипов).
//!   4. Возвращает найденные события — вызывающий код сам решает,
//!      что с ними делать (отправить в `World::events`, проиграть
//!      звук, заспавнить частицы и т.д.).
//!
//! # Почему side-table, а не поле в AnimationPlayer
//!
//! `prev_time` — чисто рантаймовая величина. Он не должен попадать
//! в сериализацию сцены и не должен восстанавливаться при load.
//! `AnimationPlayer` уже сериализуется через `AnimationSnapshot` —
//! добавлять туда transient-поле значит создавать рассинхрон.
//!
//! Side-table в `AnimationRuntime` решает это чисто: HashMap живёт
//! в DemoGame рядом с другими runtime-структурами, entity-keys
//! автоматически «забываются», когда entity исчезает из World
//! (см. `retain_alive` в конце `advance_all`).
//!
//! # Ограничения
//!
//! Событие в момент `t = 0.0` **не срабатывает** никогда: интервал
//! `(prev_time, new_time]` открыт слева. Если нужно поведение «на
//! старте» — ставь событие на `t = 0.001` или обрабатывай старт
//! анимации отдельно (например, через сравнение `prev_time == 0.0`).
//!
//! При очень большом `dt` (длиннее одного периода клипа) события
//! внутри пропущенных периодов теряются. Это осознанно: догонять
//! несколько секунд анимационных событий — почти всегда баг, а не
//! фича.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::ecs::{Entity, World};
use crate::game::components::{AnimationPlayer, SkeletonHandle};
use crate::render::skinning::{AnimationClip, Skeleton};
use crate::render::Renderer;

// ============================================================
// Публичные типы
// ============================================================

/// Маркер на таймлайне клипа.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnimationEvent {
    /// Момент срабатывания, в секундах от начала клипа.
    /// Должен быть в диапазоне `(0.0, duration]`.
    pub time: f32,
    /// Имя события. Соглашение — snake_case: "footstep", "hit".
    pub name: String,
    /// Произвольная строка-параметр. Например, имя звука
    /// ("footstep_grass") или идентификатор точки ("muzzle").
    #[serde(default)]
    pub payload: Option<String>,
}

impl AnimationEvent {
    pub fn new(time: f32, name: impl Into<String>) -> Self {
        Self { time, name: name.into(), payload: None }
    }
    pub fn with_payload(mut self, p: impl Into<String>) -> Self {
        self.payload = Some(p.into());
        self
    }
}

/// Событие, сработавшее в этом кадре.
///
/// Возвращается `AnimationRuntime::advance_all`. Вызывающий код либо
/// обрабатывает их напрямую, либо пересылает в `World::events`
/// для систем.
#[derive(Debug, Clone)]
pub struct AnimationEventTriggered {
    pub entity: Entity,
    pub clip: String,
    pub name: String,
    pub payload: Option<String>,
}

/// Реестр событий: `clip_name → [AnimationEvent]`.
///
/// Не привязан к ECS. Хранится в игре (DemoGame) и передаётся в
/// `AnimationRuntime::advance_all`.
#[derive(Debug, Default, Clone)]
pub struct AnimationEvents {
    by_clip: HashMap<String, Vec<AnimationEvent>>,
}

impl AnimationEvents {
    pub fn new() -> Self { Self::default() }

    /// Добавить событие к клипу. Список автоматически сортируется
    /// по `time` — `advance_all` рассчитывает, что события
    /// упорядочены.
    pub fn add(&mut self, clip: impl Into<String>, event: AnimationEvent) {
        let list = self.by_clip.entry(clip.into()).or_default();
        list.push(event);
        list.sort_by(|a, b| {
            a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    /// Билдер.
    pub fn with(mut self, clip: impl Into<String>, event: AnimationEvent) -> Self {
        self.add(clip, event);
        self
    }

    pub fn get(&self, clip: &str) -> &[AnimationEvent] {
        self.by_clip.get(clip).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool { self.by_clip.is_empty() }

    /// Число зарегистрированных клипов (не событий).
    pub fn clip_count(&self) -> usize { self.by_clip.len() }

    /// Загрузить из RON-файла.
    ///
    /// Формат:
    /// ```ron
    /// (
    ///     by_clip: {
    ///         "Walk": [
    ///             (time: 0.3, name: "footstep", payload: Some("footstep_grass")),
    ///             (time: 0.8, name: "footstep", payload: Some("footstep_grass")),
    ///         ],
    ///         "Attack": [
    ///             (time: 0.15, name: "swing", payload: None),
    ///             (time: 0.42, name: "hit",   payload: None),
    ///         ],
    ///     },
    /// )
    /// ```
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("read {}", path.as_ref().display()))?;
        let file: AnimationEventsFile = ron::from_str(&text)
            .with_context(|| format!("parse {}", path.as_ref().display()))?;
        let mut events = Self::new();
        for (clip, list) in file.by_clip {
            for ev in list {
                events.add(clip.clone(), ev);
            }
        }
        Ok(events)
    }

    /// Сохранить в RON-файл.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let file = AnimationEventsFile { by_clip: self.by_clip.clone() };
        let text = ron::ser::to_string_pretty(&file, ron::ser::PrettyConfig::default())
            .context("serialize AnimationEvents")?;
        std::fs::write(path.as_ref(), text)
            .with_context(|| format!("write {}", path.as_ref().display()))?;
        Ok(())
    }
}

/// Формат RON-файла. Публичный — на случай, если пользователь захочет
/// читать/писать файл вручную через `ron::from_str`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AnimationEventsFile {
    #[serde(default)]
    pub by_clip: HashMap<String, Vec<AnimationEvent>>,
}

// ============================================================
// Runtime
// ============================================================

/// Рантайм анимаций: продвигает время, обновляет скелеты, эмитит
/// события.
///
/// Хранит side-table `prev_times` — время на предыдущем кадре для
/// каждой entity. Не сериализуется, не привязан к ECS.
pub struct AnimationRuntime {
    /// `Entity → prev_time`. Записи для сущностей без
    /// `AnimationPlayer` автоматически удаляются.
    prev_times: HashMap<Entity, f32>,
}

impl AnimationRuntime {
    pub fn new() -> Self {
        Self { prev_times: HashMap::new() }
    }

    /// Продвинуть все анимации и вернуть сработавшие события.
    ///
    /// `renderer` нужен для `update_skeleton`. `clips` и `skeletons`
    /// — словари из `gltf_loader` (или откуда угодно). `events` —
    /// реестр анимационных событий.
    ///
    /// Если у entity нет `SkeletonHandle` или его скелет/клип не
    /// найден — entity пропускается, но время всё равно продвигается,
    /// чтобы избежать «прыжка» при догрузке.
    pub fn advance_all(
        &mut self,
        world: &mut World,
        renderer: &mut Renderer,
        clips: &HashMap<String, AnimationClip>,
        skeletons: &HashMap<String, Skeleton>,
        events: &AnimationEvents,
        dt: f32,
    ) -> Vec<AnimationEventTriggered> {
        let entities: Vec<Entity> = world
            .query::<AnimationPlayer>()
            .map(|(e, _)| e)
            .collect();

        let mut triggered: Vec<AnimationEventTriggered> = Vec::new();

        for e in &entities {
            let e = *e;

            // 1. Продвигаем время. Читаем поля заранее, чтобы не
            //    держать borrow World при дальнейших чтениях.
            let (clip_name, looping, new_time) = {
                let Some(player) = world.get_mut::<AnimationPlayer>(e) else {
                    continue;
                };
                player.time += dt * player.speed;
                (player.clip.clone(), player.looping, player.time)
            };

            // 2. prev_time — из side-table. Для новой entity
            //    считаем prev_time == 0.0.
            let prev_time = self.prev_times.get(&e).copied().unwrap_or(0.0);

            // 3. Находим клип.
            let Some(clip) = clips.get(&clip_name) else {
                // Клип не загружен — prev_time всё равно обновляем,
                // чтобы при догрузке не выстрелить пачкой старых
                // событий.
                self.prev_times.insert(e, new_time);
                continue;
            };
            let duration = clip.duration;

            // 4. Границы интервала (from, to]. Для looping обёрнуты
            //    в [0, duration).
            let from_time = if looping && duration > 0.0 {
                prev_time % duration
            } else {
                prev_time.min(duration)
            };
            let to_time = if looping && duration > 0.0 {
                new_time % duration
            } else {
                new_time.min(duration)
            };

            // 5. Эмитим события.
            for ev in events.get(&clip_name) {
                if event_in_range(ev.time, from_time, to_time, looping, duration) {
                    triggered.push(AnimationEventTriggered {
                        entity: e,
                        clip: clip_name.clone(),
                        name: ev.name.clone(),
                        payload: ev.payload.clone(),
                    });
                }
            }

            // 6. Обновляем скелет.
            let skel_handle = world.get::<SkeletonHandle>(e).cloned();
            if let Some(handle) = skel_handle {
                if let Some(skel) = skeletons.get(&handle.0) {
                    let final_time = if looping && duration > 0.0 {
                        to_time
                    } else {
                        new_time.min(duration)
                    };
                    let local_pose = clip.local_pose(final_time, &skel.local_bind);
                    let joint_matrices = skel.joint_matrices(&local_pose);
                    renderer.update_skeleton(&handle.0, &joint_matrices);
                }
            }

            // 7. Запоминаем new_time для следующего кадра.
            self.prev_times.insert(e, new_time);
        }

        // 8. Чистим записи для entity, которых больше нет.
        let alive_set: std::collections::HashSet<Entity> =
            entities.iter().copied().collect();
        self.prev_times.retain(|k, _| alive_set.contains(k));

        triggered
    }

    /// Сбросить историю для конкретной entity. Полезно после телепорта
    /// или смены клипа: иначе возможен «прыжок» через большой
    /// интервал — и пачка событий за один кадр.
    pub fn reset(&mut self, entity: Entity) {
        self.prev_times.remove(&entity);
    }

    /// Полный сброс.
    pub fn clear(&mut self) {
        self.prev_times.clear();
    }

    /// Сколько entity сейчас отслеживается. Для дебага.
    pub fn tracked_count(&self) -> usize {
        self.prev_times.len()
    }
}

impl Default for AnimationRuntime {
    fn default() -> Self { Self::new() }
}

// ============================================================
// Helpers
// ============================================================

/// Попадает ли `event_time` в интервал `(from, to]` с учётом wrap.
///
/// * `from <= to` — обычный случай, интервал без wrap.
/// * `from > to` и `looping` — интервал = `(from, duration] ∪ [0, to]`.
/// * `from > to` и не looping — не должно случаться; игнорируем.
fn event_in_range(
    event_time: f32,
    from: f32,
    to: f32,
    looping: bool,
    duration: f32,
) -> bool {
    if duration <= 0.0 { return false; }

    if from <= to {
        event_time > from && event_time <= to
    } else if looping {
        event_time > from || event_time <= to
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_in_range_simple() {
        assert!( event_in_range(0.5, 0.0, 1.0, false, 2.0));
        assert!( event_in_range(1.0, 0.0, 1.0, false, 2.0));
        assert!(!event_in_range(0.0, 0.0, 1.0, false, 2.0)); // граница слева
        assert!(!event_in_range(1.5, 0.0, 1.0, false, 2.0));
    }

    #[test]
    fn event_in_range_wrap() {
        // Клип 2.0s, prev = 1.8, new = 2.3 → wrap: to = 0.3.
        assert!(event_in_range(1.9, 1.8, 0.3, true, 2.0));
        assert!(event_in_range(2.0, 1.8, 0.3, true, 2.0));
        assert!(event_in_range(0.1, 1.8, 0.3, true, 2.0));
        assert!(!event_in_range(1.0, 1.8, 0.3, true, 2.0));
        assert!(!event_in_range(0.5, 1.8, 0.3, true, 2.0));
    }

    #[test]
    fn events_registry_sorts() {
        let mut ev = AnimationEvents::new();
        ev.add("Walk", AnimationEvent::new(0.8, "b"));
        ev.add("Walk", AnimationEvent::new(0.3, "a"));
        ev.add("Walk", AnimationEvent::new(1.2, "c"));
        let list = ev.get("Walk");
        assert_eq!(list[0].name, "a");
        assert_eq!(list[1].name, "b");
        assert_eq!(list[2].name, "c");
    }

    #[test]
    fn events_registry_empty_clip() {
        let ev = AnimationEvents::new();
        assert!(ev.get("Nonexistent").is_empty());
        assert!(ev.is_empty());
    }
}