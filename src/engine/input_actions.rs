//! Input actions: семантические действия поверх raw-клавиш.
//!
//! # Зачем
//!
//! Игровой код не должен знать, что «прыжок» — это Space. Он должен
//! знать, что есть действие `jump`. Тогда:
//!   * одна и та же логика работает при разных раскладках;
//!   * ребинд делается в одном месте;
//!   * контексты («в меню», «в игре», «в редакторе») отделяют
//!     несовместимые действия.
//!
//! # Что внутри
//!
//! `InputMap` — таблица `action → [bindings]`. Действие активно,
//! если его контекст входит в `active_contexts` (или у действия нет
//! контекста — тогда оно всегда активно).
//!
//! `pressed`/`down`/`released` — обёртки над `Input`, которые
//! проверяют все привязки действия.
//!
//! # Save / load (Фаза 6.2)
//!
//! `InputMap::save(path)` / `load(path)` — сериализация в RON.
//! Клавиши хранятся через `Key` (сериализуемый аналог `winit::KeyCode`,
//! см. `input_keys.rs`). Кнопки мыши — через `MouseBtn`.
//!
//! Формат файла `input.ron`:
//! ```ron
//! (
//!     bindings: {
//!         "jump":   [Key(Space), Key(KeyZ)],
//!         "fire":   [Mouse(Left)],
//!         "pause":  [Key(Escape)],
//!     },
//!     contexts: {
//!         "jump":  ["gameplay"],
//!         "fire":  ["gameplay"],
//!         "pause": ["gameplay"],
//!     },
//! )
//! ```
//!
//! Активные контексты **не** сохраняются — они runtime.
//!
//! # Пример
//!
//! ```ignore
//! // В `configure_input`:
//! map.bind("jump", Key::Space)
//!    .bind("jump", Key::KeyZ)
//!    .bind_mouse("fire", MouseBtn::Left);
//! map.action_in_context("jump", "gameplay");
//!
//! // В update:
//! if input.pressed("jump") { /* прыжок */ }
//! if input.down("fire")    { /* автоогонь */ }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::input::Input;
use super::input_keys::Key;

/// Сериализуемый аналог `winit::event::MouseButton`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseBtn {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

impl MouseBtn {
    pub fn from_winit(b: winit::event::MouseButton) -> Option<Self> {
        Some(match b {
            winit::event::MouseButton::Left => Self::Left,
            winit::event::MouseButton::Right => Self::Right,
            winit::event::MouseButton::Middle => Self::Middle,
            winit::event::MouseButton::Back => Self::Back,
            winit::event::MouseButton::Forward => Self::Forward,
            _ => return None,
        })
    }

    pub fn to_winit(self) -> winit::event::MouseButton {
        match self {
            Self::Left => winit::event::MouseButton::Left,
            Self::Right => winit::event::MouseButton::Right,
            Self::Middle => winit::event::MouseButton::Middle,
            Self::Back => winit::event::MouseButton::Back,
            Self::Forward => winit::event::MouseButton::Forward,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Left => "LMB",
            Self::Right => "RMB",
            Self::Middle => "MMB",
            Self::Back => "MB4",
            Self::Forward => "MB5",
        }
    }
}

/// Одна привязка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Binding {
    Key(Key),
    Mouse(MouseBtn),
}

impl Binding {
    pub fn display_name(self) -> &'static str {
        match self {
            Binding::Key(k) => k.display_name(),
            Binding::Mouse(m) => m.display_name(),
        }
    }
}

/// RON-структура для save/load. Публичная — чтобы пользователь мог
/// писать файл вручную через `ron::to_string`/`from_str`.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct InputMapFile {
    #[serde(default)]
    pub bindings: HashMap<String, Vec<Binding>>,
    #[serde(default)]
    pub contexts: HashMap<String, Vec<String>>,
}

/// Таблица действий.
///
/// Действие — именованная группа привязок. Активно, если активен
/// хотя бы один из его контекстов (или если контекстов нет).
#[derive(Debug, Default, Clone)]
pub struct InputMap {
    bindings: HashMap<String, Vec<Binding>>,
    /// `action → contexts`. Пустое множество = всегда активно.
    contexts: HashMap<String, HashSet<String>>,
    /// Активные контексты кадра. Runtime, не сохраняется.
    active_contexts: HashSet<String>,
}

impl InputMap {
    pub fn new() -> Self {
        Self::default()
    }

    // ============================================================
    // Регистрация
    // ============================================================

    /// Привязать клавишу к действию.
    pub fn bind(&mut self, action: impl Into<String>, key: Key) -> &mut Self {
        let action = action.into();
        let list = self.bindings.entry(action).or_default();
        let b = Binding::Key(key);
        if !list.contains(&b) {
            list.push(b);
        }
        self
    }

    /// Привязать кнопку мыши к действию.
    pub fn bind_mouse(&mut self, action: impl Into<String>, button: MouseBtn) -> &mut Self {
        let action = action.into();
        let list = self.bindings.entry(action).or_default();
        let b = Binding::Mouse(button);
        if !list.contains(&b) {
            list.push(b);
        }
        self
    }

    /// Указать, что действие активно только в контексте `ctx`.
    pub fn action_in_context(
        &mut self,
        action: impl Into<String>,
        ctx: impl Into<String>,
    ) -> &mut Self {
        self.contexts
            .entry(action.into())
            .or_default()
            .insert(ctx.into());
        self
    }

    /// Удалить все привязки действия. Контекст остаётся.
    pub fn unbind_all(&mut self, action: &str) {
        if let Some(list) = self.bindings.get_mut(action) {
            list.clear();
        }
    }

    /// Полностью удалить действие.
    pub fn remove_action(&mut self, action: &str) {
        self.bindings.remove(action);
        self.contexts.remove(action);
    }

    // ============================================================
    // Ребинд (Фаза 6.2)
    // ============================================================

    /// Заменить привязку #`index` действия `action` на `new_binding`.
    /// Если `index` вне диапазона — добавляет в конец.
    ///
    /// Возвращает `true`, если что-то изменилось.
    pub fn rebind(&mut self, action: &str, index: usize, new_binding: Binding) -> bool {
        let list = self.bindings.entry(action.to_string()).or_default();
        // Если новая привязка уже где-то есть — не дублируем (просто
        // удаляем старую на этой позиции).
        if list.contains(&new_binding) {
            if index < list.len() {
                list.remove(index);
            }
            return true;
        }
        if index < list.len() {
            list[index] = new_binding;
        } else {
            list.push(new_binding);
        }
        true
    }

    /// Добавить привязку к действию, не удаляя существующие.
    /// Возвращает `false`, если такая привязка уже была.
    pub fn add_binding(&mut self, action: &str, new_binding: Binding) -> bool {
        let list = self.bindings.entry(action.to_string()).or_default();
        if list.contains(&new_binding) {
            return false;
        }
        list.push(new_binding);
        true
    }

    /// Удалить все привязки с этой клавишей/кнопкой во **всех**
    /// действиях, кроме `except_action`. Полезно при ребинде:
    /// если пользователь назначает `Space` на `jump`, надо убрать
    /// `Space` с `crouch`, иначе они конфликтуют.
    ///
    /// Возвращает количество удалённых привязок.
    pub fn remove_binding_everywhere(&mut self, b: Binding, except_action: &str) -> usize {
        let mut removed = 0;
        for (action, list) in self.bindings.iter_mut() {
            if action == except_action { continue; }
            let before = list.len();
            list.retain(|x| *x != b);
            removed += before - list.len();
        }
        removed
    }

    /// Список привязок действия (копия).
    pub fn get_bindings(&self, action: &str) -> Vec<Binding> {
        self.bindings.get(action).cloned().unwrap_or_default()
    }

    // ============================================================
    // Save / load (Фаза 6.2)
    // ============================================================

    pub fn to_file_data(&self) -> InputMapFile {
        let contexts = self.contexts
            .iter()
            .map(|(a, cs)| {
                let mut v: Vec<String> = cs.iter().cloned().collect();
                v.sort();
                (a.clone(), v)
            })
            .collect();
        InputMapFile {
            bindings: self.bindings.clone(),
            contexts,
        }
    }

    pub fn from_file_data(data: InputMapFile) -> Self {
        let contexts = data.contexts
            .into_iter()
            .map(|(a, cs)| (a, cs.into_iter().collect()))
            .collect();
        Self {
            bindings: data.bindings,
            contexts,
            active_contexts: HashSet::new(),
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        let data = self.to_file_data();
        let text = ron::ser::to_string_pretty(&data, ron::ser::PrettyConfig::default())
            .context("serialize InputMap")?;
        std::fs::write(path.as_ref(), text)
            .with_context(|| format!("write {}", path.as_ref().display()))?;
        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("read {}", path.as_ref().display()))?;
        let data: InputMapFile = ron::from_str(&text)
            .with_context(|| format!("parse {}", path.as_ref().display()))?;
        Ok(Self::from_file_data(data))
    }

    /// Загрузить из файла, если он есть; иначе вернуть `default`.
    /// Логирует warning при ошибке разбора, но не падает.
    pub fn load_or_default(path: impl AsRef<Path>, default: Self) -> Self {
        let p = path.as_ref();
        if !p.exists() {
            return default;
        }
        match Self::load(p) {
            Ok(m) => {
                log::info!("InputMap loaded from {}", p.display());
                m
            }
            Err(e) => {
                log::warn!("Failed to load InputMap from {}: {}", p.display(), e);
                default
            }
        }
    }

    // ============================================================
    // Контексты
    // ============================================================

    pub fn push_context(&mut self, ctx: impl Into<String>) {
        self.active_contexts.insert(ctx.into());
    }

    pub fn pop_context(&mut self, ctx: &str) {
        self.active_contexts.remove(ctx);
    }

    pub fn clear_contexts(&mut self) {
        self.active_contexts.clear();
    }

    pub fn is_context_active(&self, ctx: &str) -> bool {
        self.active_contexts.contains(ctx)
    }

    pub fn is_action_active(&self, action: &str) -> bool {
        match self.contexts.get(action) {
            None => true,
            Some(cs) if cs.is_empty() => true,
            Some(cs) => cs.iter().any(|c| self.active_contexts.contains(c)),
        }
    }

    // ============================================================
    // Запросы
    // ============================================================

    pub fn pressed(&self, action: &str, input: &Input) -> bool {
        if !self.is_action_active(action) { return false; }
        let Some(list) = self.bindings.get(action) else { return false };
        list.iter().any(|b| match b {
            Binding::Key(k) => k.to_keycode().map_or(false, |kc| input.key_pressed(kc)),
            Binding::Mouse(m) => input.mouse_pressed(m.to_winit()),
        })
    }

    pub fn down(&self, action: &str, input: &Input) -> bool {
        if !self.is_action_active(action) { return false; }
        let Some(list) = self.bindings.get(action) else { return false };
        list.iter().any(|b| match b {
            Binding::Key(k) => k.to_keycode().map_or(false, |kc| input.key_down(kc)),
            Binding::Mouse(m) => input.mouse_down(m.to_winit()),
        })
    }

    pub fn released(&self, action: &str, input: &Input) -> bool {
        if !self.is_action_active(action) { return false; }
        let Some(list) = self.bindings.get(action) else { return false };
        list.iter().any(|b| match b {
            Binding::Key(k) => k.to_keycode().map_or(false, |kc| input.key_released(kc)),
            Binding::Mouse(m) => input.mouse_released(m.to_winit()),
        })
    }

    // ============================================================
    // Отладка / UI
    // ============================================================

    pub fn action_names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.bindings.keys().map(|s| s.as_str()).collect();
        v.sort();
        v
    }

    pub fn binding_count(&self, action: &str) -> usize {
        self.bindings.get(action).map(|v| v.len()).unwrap_or(0)
    }

    pub fn has_action(&self, action: &str) -> bool {
        self.bindings.contains_key(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_gating() {
        let mut map = InputMap::new();
        map.bind("pause", Key::Escape);
        map.action_in_context("pause", "gameplay");

        assert!(!map.is_action_active("pause"));
        map.push_context("gameplay");
        assert!(map.is_action_active("pause"));
        map.pop_context("gameplay");
        assert!(!map.is_action_active("pause"));
    }

    #[test]
    fn action_without_context_always_active() {
        let mut map = InputMap::new();
        map.bind("help", Key::F1);

        assert!(map.is_action_active("help"));
        map.push_context("menu");
        assert!(map.is_action_active("help"));
    }

    #[test]
    fn multiple_contexts_per_action() {
        let mut map = InputMap::new();
        map.bind("confirm", Key::Enter);
        map.action_in_context("confirm", "menu");
        map.action_in_context("confirm", "dialogue");

        assert!(!map.is_action_active("confirm"));
        map.push_context("menu");
        assert!(map.is_action_active("confirm"));
        map.clear_contexts();
        map.push_context("dialogue");
        assert!(map.is_action_active("confirm"));
    }

    #[test]
    fn binding_registration() {
        let mut map = InputMap::new();
        map.bind("jump", Key::Space);
        map.bind("jump", Key::KeyZ);
        map.bind_mouse("fire", MouseBtn::Left);

        assert_eq!(map.binding_count("jump"), 2);
        assert_eq!(map.binding_count("fire"), 1);
        assert_eq!(map.binding_count("nonexistent"), 0);

        map.bind("jump", Key::Space);
        assert_eq!(map.binding_count("jump"), 2);
    }

    #[test]
    fn rebind_replaces_at_index() {
        let mut map = InputMap::new();
        map.bind("jump", Key::Space);
        map.bind("jump", Key::KeyZ);

        map.rebind("jump", 0, Binding::Key(Key::KeyJ));
        let list = map.get_bindings("jump");
        assert_eq!(list[0], Binding::Key(Key::KeyJ));
        assert_eq!(list[1], Binding::Key(Key::KeyZ));
    }

    #[test]
    fn rebind_removes_duplicates() {
        let mut map = InputMap::new();
        map.bind("jump", Key::Space);
        map.bind("jump", Key::KeyZ);

        // Делаем `jump` на Space и Space — второй становится
        // пустым (удаляется).
        map.rebind("jump", 1, Binding::Key(Key::Space));
        // list[0] = Space, list[1] удалён (Space уже есть).
        assert_eq!(map.binding_count("jump"), 1);
    }

    #[test]
    fn remove_binding_everywhere() {
        let mut map = InputMap::new();
        map.bind("jump", Key::Space);
        map.bind("crouch", Key::Space);
        map.bind("fire", Key::Space);

        let removed = map.remove_binding_everywhere(Binding::Key(Key::Space), "jump");
        assert_eq!(removed, 2);
        assert_eq!(map.binding_count("jump"), 1);
        assert_eq!(map.binding_count("crouch"), 0);
        assert_eq!(map.binding_count("fire"), 0);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut map = InputMap::new();
        map.bind("jump", Key::Space);
        map.bind("jump", Key::KeyZ);
        map.bind_mouse("fire", MouseBtn::Left);
        map.action_in_context("jump", "gameplay");
        map.action_in_context("fire", "gameplay");

        let data = map.to_file_data();
        let ron_text = ron::ser::to_string(&data).expect("serialize");
        let parsed: InputMapFile = ron::from_str(&ron_text).expect("deserialize");
        let restored = InputMap::from_file_data(parsed);

        assert_eq!(restored.binding_count("jump"), 2);
        assert_eq!(restored.binding_count("fire"), 1);
        // Контексты сохранены.
        assert!(restored.is_action_active("jump") == false); // активных нет
        // Но конфиг контекста есть — при push он сработает.
        let mut restored = restored;
        restored.push_context("gameplay");
        assert!(restored.is_action_active("jump"));
    }

    #[test]
    fn action_registry() {
        let mut map = InputMap::new();
        map.bind("b", Key::KeyB);
        map.bind("a", Key::KeyA);
        map.bind("c", Key::KeyC);

        assert_eq!(map.action_names(), vec!["a", "b", "c"]);
        assert!(map.has_action("a"));
        assert!(!map.has_action("z"));

        map.remove_action("a");
        assert!(!map.has_action("a"));
    }
}