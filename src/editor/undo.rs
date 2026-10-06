//! Undo/redo стек.
//!
//! Каждый снапшот — пара `(scene_ron, game_state_ron)`. `scene_ron`
//! собирается движком (`save_scene_with_assets_to_string`), а
//! `game_state_ron` — это непрозрачная для движка RON-строка
//! (`Game::save_game_state`).
//!
//! Если `Game::save_game_state` возвращает `None` — снапшот содержит
//! `None`, и при откате `App` ничего не передаёт в `load_game_state`.
//!
//! ## Про game_state
//!
//! ИЗМЕНЕНО (правка undo #1): RON и game_state **не смешиваются**.
//!
//! Раньше `push_forced` вшивал game_state в RON через
//! `save_scene_with_game_state_to_string`, но при `undo()`
//! использовал `load_scene_with_assets_from_str` (не `_full`) —
//! и вшитый state терялся. Плюс поле `Snapshot::game_state_ron`
//! жёстко ставилось в `None`.
//!
//! Теперь:
//!   * RON содержит только мир + материалы (`save_scene_with_assets_to_string`);
//!   * game_state живёт **отдельно** в поле `Snapshot::game_state_ron`;
//!   * `undo()`/`redo()` возвращают именно это поле;
//!   * для обратной совместимости используется
//!     `load_scene_with_assets_from_str_full` — если во входном RON
//!     оказался вшитый game_state (старая история в памяти),
//!     он тоже подхватится.

use std::collections::VecDeque;
use std::time::Instant;

use crate::ecs::World;
use crate::render::Renderer;

const MAX_UNDO: usize = 50;

/// Cooldown защищает от шторма правок (drag). Если пользователь
/// явно вызвал операцию (AddCube, Delete) — она идёт через
/// `push_forced` без cooldown.
const PUSH_COOLDOWN_MS: u128 = 300;

/// Один снапшот: сериализованный мир + game-specific состояние.
///
/// ИЗМЕНЕНО (undo #1): `scene_ron` **не содержит** game_state —
/// он хранится отдельно в `game_state_ron`. Это устраняет
/// рассогласование между содержимым RON и полем.
struct Snapshot {
    scene_ron: String,
    game_state_ron: Option<String>,
}

pub struct UndoStack {
    undo: VecDeque<Snapshot>,
    redo: Vec<Snapshot>,
    last_push: Instant,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            undo: VecDeque::with_capacity(MAX_UNDO),
            redo: Vec::new(),
            // Инициализируем «в прошлом», чтобы первый push не был
            // заблокирован cooldown'ом.
            last_push: Instant::now() - std::time::Duration::from_secs(3600),
        }
    }

    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }

    /// Проверка, истёк ли cooldown — используется в `App::redraw`,
    /// чтобы решить, снимать ли дорогой snapshot до egui.
    pub fn can_push_now(&self) -> bool {
        self.last_push.elapsed().as_millis() >= PUSH_COOLDOWN_MS
    }

    /// Cooldown-версия. Возвращает `false`, если снапшот **не** был
    /// положен (из-за cooldown или ошибки сериализации).
    pub fn push(
        &mut self,
        world: &World,
        renderer: &Renderer,
        game_state_ron: Option<String>,
    ) -> bool {
        if !self.can_push_now() {
            return false;
        }
        self.push_forced(world, renderer, game_state_ron)
    }

    /// Принудительный снапшот без cooldown. Для явных операций:
    /// AddCube, Delete, Duplicate, Paste, MakeUnique, …
    ///
    /// ИЗМЕНЕНО (undo #1): RON — только мир + материалы. `game_state_ron`
    /// кладётся отдельно в поле. Раньше здесь был
    /// `save_scene_with_game_state_to_string`, но `undo()` использовал
    /// не-`_full` загрузчик и терял вшитый state.
    pub fn push_forced(
        &mut self,
        world: &World,
        renderer: &Renderer,
        game_state_ron: Option<String>,
    ) -> bool {
        match crate::scene::save_scene_with_assets_to_string(world, renderer, None) {
            Ok(s) => {
                self.push_snapshot_with_state(s, game_state_ron);
                true
            }
            Err(e) => {
                log::warn!("undo push failed: {}", e);
                false
            }
        }
    }

    /// Готовый снапшот сцены + game_state. Тонкая обёртка над
    /// `push_snapshot_with_state` с cooldown-проверкой.
    ///
    /// Используется `App::redraw`: pre-UI snapshot уже снят
    /// (без game_state) — здесь дополняется актуальным состоянием.
    pub fn push_snapshot(
        &mut self,
        snapshot: String,
        game_state_ron: Option<String>,
    ) -> bool {
        if !self.can_push_now() {
            return false;
        }
        self.push_snapshot_with_state(snapshot, game_state_ron);
        true
    }

    /// Единая точка добавления снапшота с game_state.
    /// `scene_ron` **не должен** содержать game_state — он живёт отдельно.
    fn push_snapshot_with_state(
        &mut self,
        scene_ron: String,
        game_state_ron: Option<String>,
    ) {
        if self.undo.len() >= MAX_UNDO {
            self.undo.pop_front();
        }
        self.undo.push_back(Snapshot {
            scene_ron,
            game_state_ron,
        });
        self.redo.clear();
        self.last_push = Instant::now();
    }

    /// Восстановить предыдущее состояние. Возвращает `(World, game_state)`
    /// и синхронно восстанавливает материалы в `Renderer`.
    ///
    /// `current_game_state` — game-state **текущего** мира, нужен для
    /// помещения в redo.
    pub fn undo(
        &mut self,
        world: &World,
        renderer: &mut Renderer,
        current_game_state: Option<String>,
    ) -> Option<(World, Option<String>)> {
        let prev = self.undo.pop_back()?;

        // Текущее состояние → в redo. RON — только мир, game_state — в поле.
        if let Ok(current) =
            crate::scene::save_scene_with_assets_to_string(world, renderer, None)
        {
            self.redo.push(Snapshot {
                scene_ron: current,
                game_state_ron: current_game_state,
            });
        }

        // ИСПРАВЛЕНО (undo #1): используем `_full`, чтобы подстраховаться
        // от снапшотов, созданных старой версией кода (где game_state
        // вшивался в RON, а поле было `None`). Приоритет — поле.
        match crate::scene::load_scene_with_assets_from_str_full(&prev.scene_ron, renderer) {
            Ok((mut w, _spawn, embedded_gs)) => {
                w.sync_next_id();
                Some((w, prev.game_state_ron.or(embedded_gs)))
            }
            Err(e) => {
                log::warn!("undo load failed: {}", e);
                None
            }
        }
    }

    pub fn redo(
        &mut self,
        world: &World,
        renderer: &mut Renderer,
        current_game_state: Option<String>,
    ) -> Option<(World, Option<String>)> {
        let next = self.redo.pop()?;

        if let Ok(current) =
            crate::scene::save_scene_with_assets_to_string(world, renderer, None)
        {
            if self.undo.len() >= MAX_UNDO {
                self.undo.pop_front();
            }
            self.undo.push_back(Snapshot {
                scene_ron: current,
                game_state_ron: current_game_state,
            });
        }

        match crate::scene::load_scene_with_assets_from_str_full(&next.scene_ron, renderer) {
            Ok((mut w, _spawn, embedded_gs)) => {
                w.sync_next_id();
                Some((w, next.game_state_ron.or(embedded_gs)))
            }
            Err(e) => {
                log::warn!("redo load failed: {}", e);
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

impl Default for UndoStack {
    fn default() -> Self { Self::new() }
}