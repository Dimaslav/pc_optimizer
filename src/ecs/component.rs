//! Плотное хранилище компонентов: элементы лежат в Vec<T>,
//! сущности — в параллельном Vec<Entity>, обратный индекс — HashMap.

use std::any::Any;
use std::collections::HashMap;

pub struct ComponentStorage<T> {
    data: Vec<T>,
    entities: Vec<Entity>,
    lookup: HashMap<Entity, usize>,
}

impl<T> Default for ComponentStorage<T> {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            entities: Vec::new(),
            lookup: HashMap::new(),
        }
    }
}

impl<T> ComponentStorage<T> {
    pub fn insert(&mut self, e: Entity, c: T) -> Option<T> {
        if let Some(&idx) = self.lookup.get(&e) {
            return Some(std::mem::replace(&mut self.data[idx], c));
        }
        let idx = self.data.len();
        self.data.push(c);
        self.entities.push(e);
        self.lookup.insert(e, idx);
        None
    }

    pub fn remove(&mut self, e: Entity) -> Option<T> {
        let idx = self.lookup.remove(&e)?;
        self.entities.swap_remove(idx);
        let removed = self.data.swap_remove(idx);
        // Если swap_remove переместил последний элемент на место idx —
        // обновляем его индекс в lookup.
        if idx < self.entities.len() {
            self.lookup.insert(self.entities[idx], idx);
        }
        Some(removed)
    }

    pub fn get(&self, e: Entity) -> Option<&T> {
        self.lookup.get(&e).map(|&i| &self.data[i])
    }

    pub fn get_mut(&mut self, e: Entity) -> Option<&mut T> {
        self.lookup.get(&e).map(|&i| &mut self.data[i])
    }

    pub fn contains(&self, e: Entity) -> bool {
        self.lookup.contains_key(&e)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.entities.iter().copied().zip(self.data.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.entities.iter().copied().zip(self.data.iter_mut())
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Ссылка на внутренний список entity без копирования.
    ///
    /// ИЗМЕНЕНО (#9): используется `World::for_each_pair` для заполнения
    /// скретч-буфера — без промежуточной аллокации `Vec<Entity>`.
    ///
    /// Раньше здесь был `entity_list() -> Vec<Entity>` с `.clone()`,
    /// который аллоцировал на каждый вызов `for_each_pair` (то есть на
    /// каждый кадр в `MovementSystem::update`).
    ///
    /// Возвращаемый `slice::Iter` живёт ровно столько, сколько живёт
    /// borrow `self`. Клонирование больше не нужно: caller сам решает,
    /// куда сложить результат (в скретч-буфер World).
    pub fn entities_iter(&self) -> std::slice::Iter<'_, Entity> {
        self.entities.iter()
    }

    /// Оставлено для обратной совместимости внешнего API.
    ///
    /// **Внутри движка не используется** — `World::for_each_pair`
    /// предпочитает `entities_iter()` + скретч-буфер. Метод сохранён
    /// публичным, потому что `ComponentStorage` экспортируется из
    /// `ecs::mod` и может использоваться извне.
    pub fn entity_list(&self) -> Vec<Entity> {
        self.entities.clone()
    }
}

use super::world::Entity;

// === Type-erased trait, чтобы хранить разные ComponentStorage<T> в одной HashMap ===

pub trait AnyStorage: Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn remove_entity(&mut self, e: Entity);
    fn contains_entity(&self, e: Entity) -> bool;
    fn len(&self) -> usize;
}

impl<T: Send + Sync + 'static> AnyStorage for ComponentStorage<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn remove_entity(&mut self, e: Entity) {
        let _ = self.remove(e);
    }
    fn contains_entity(&self, e: Entity) -> bool {
        self.contains(e)
    }
    fn len(&self) -> usize {
        ComponentStorage::len(self)
    }
}