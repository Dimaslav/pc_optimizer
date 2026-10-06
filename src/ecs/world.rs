use std::any::{Any, TypeId};
use std::collections::HashMap;

use super::component::{AnyStorage, ComponentStorage};
use super::events::Events;

pub type Entity = u32;

#[derive(Default)]
pub struct World {
    next_id: Entity,
    entities: Vec<Entity>,
    /// Entity → индекс в `entities`. Нужен для O(1) despawn.
    entity_pos: HashMap<Entity, usize>,
    storages: HashMap<TypeId, Box<dyn AnyStorage>>,
    events: Events,

    /// ИЗМЕНЕНО (#9): скретч-буфер для `for_each_pair`.
    ///
    /// Раньше при каждом вызове `for_each_pair` создавался новый
    /// `Vec<Entity>` через `ComponentStorage::entity_list()` (который
    /// делал `.clone()`). В `MovementSystem::update` это происходило
    /// каждый кадр — 60+ аллокаций/сек для сцены средней величины.
    ///
    /// Теперь: один `Vec<Entity>` живёт в `World`, `for_each_pair`
    /// вынимает его через `mem::take`, заполняет из `entities_iter()`
    /// A-стораджа, обходит, возвращает обратно. Аллокация происходит
    /// только при первом вызове (или если буфер вырос).
    ///
    /// `Default` даёт `Vec::new()` — первая итерация аллоцирует, дальше
    /// только переиспользуется.
    scratch_entities: Vec<Entity>,
}

impl World {
    pub fn new() -> Self { Self::default() }

    // ============================================================
    // Жизненный цикл сущностей
    // ============================================================

    pub fn spawn(&mut self) -> Entity {
        let id = self.next_id;
        self.next_id += 1;
        let idx = self.entities.len();
        self.entities.push(id);
        self.entity_pos.insert(id, idx);
        id
    }

    /// O(1) удаление через swap_remove + обновление обратного индекса.
    pub fn despawn(&mut self, entity: Entity) {
        let Some(pos) = self.entity_pos.remove(&entity) else {
            return;
        };
        self.entities.swap_remove(pos);
        // Если swap_remove переместил последний элемент — обновляем его индекс.
        if pos < self.entities.len() {
            self.entity_pos.insert(self.entities[pos], pos);
        }
        for store in self.storages.values_mut() {
            store.remove_entity(entity);
        }
    }

    /// Гарантирует, что `next_id` больше любого существующего entity.
    /// Вызывать после загрузки сцены / десериализации, если id вставлялись
    /// напрямую.
    pub fn sync_next_id(&mut self) {
        if let Some(&max) = self.entities.iter().max() {
            if max >= self.next_id {
                self.next_id = max.saturating_add(1);
            }
        }
    }

    // ============================================================
    // Компоненты
    // ============================================================

    pub fn insert<T: Send + Sync + 'static>(&mut self, entity: Entity, component: T) {
        let storage = self
            .storages
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Box::new(ComponentStorage::<T>::default()));
        storage
            .as_any_mut()
            .downcast_mut::<ComponentStorage<T>>()
            .unwrap()
            .insert(entity, component);
    }

    pub fn get<T: Send + Sync + 'static>(&self, entity: Entity) -> Option<&T> {
        let storage = self.storages.get(&TypeId::of::<T>())?;
        storage.as_any().downcast_ref::<ComponentStorage<T>>()?.get(entity)
    }

    pub fn get_mut<T: Send + Sync + 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        let storage = self.storages.get_mut(&TypeId::of::<T>())?;
        storage.as_any_mut().downcast_mut::<ComponentStorage<T>>()?.get_mut(entity)
    }

    pub fn remove<T: Send + Sync + 'static>(&mut self, entity: Entity) -> Option<T> {
        let storage = self.storages.get_mut(&TypeId::of::<T>())?;
        storage.as_any_mut().downcast_mut::<ComponentStorage<T>>()?.remove(entity)
    }

    pub fn has<T: Send + Sync + 'static>(&self, entity: Entity) -> bool {
        self.storages
            .get(&TypeId::of::<T>())
            .map(|s| s.contains_entity(entity))
            .unwrap_or(false)
    }

    // ============================================================
    // Запросы
    // ============================================================

    pub fn query<T: Send + Sync + 'static>(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.storages
            .get(&TypeId::of::<T>())
            .and_then(|s| s.as_any().downcast_ref::<ComponentStorage<T>>())
            .map(|s| s.iter())
            .into_iter()
            .flatten()
    }

    pub fn query_mut<T: Send + Sync + 'static>(
        &mut self,
    ) -> impl Iterator<Item = (Entity, &mut T)> {
        self.storages
            .get_mut(&TypeId::of::<T>())
            .and_then(|s| s.as_any_mut().downcast_mut::<ComponentStorage<T>>())
            .map(|s| s.iter_mut())
            .into_iter()
            .flatten()
    }

    pub fn query2<A, B>(&self) -> impl Iterator<Item = (Entity, &A, &B)>
    where
        A: Send + Sync + 'static,
        B: Send + Sync + 'static,
    {
        let a_store = self
            .storages
            .get(&TypeId::of::<A>())
            .and_then(|s| s.as_any().downcast_ref::<ComponentStorage<A>>());
        let b_store = self
            .storages
            .get(&TypeId::of::<B>())
            .and_then(|s| s.as_any().downcast_ref::<ComponentStorage<B>>());

        a_store.into_iter().flat_map(move |a| {
            a.iter().filter_map(move |(e, av)| {
                b_store.and_then(|b| b.get(e)).map(|bv| (e, av, bv))
            })
        })
    }

    /// ИЗМЕНЕНО (#9): переписано на переиспользуемый скретч-буфер.
    ///
    /// До этого `entity_list()` клонировал `Vec<Entity>` из A-стораджа
    /// **на каждый вызов**. `MovementSystem::update` дёргает этот метод
    /// каждый кадр → 60+ аллокаций/сек + memcpy. Теперь буфер живёт
    /// в `World` и переиспользуется между вызовами.
    ///
    /// Рекурсивный вызов `for_each_pair` из `f` технически возможен
    /// (хотя требует `unsafe` или передачи `World` через `RefCell`),
    /// и он тоже безопасен: `mem::take` вынимает буфер локально, а
    /// вложенный вызов получит `Vec::new()` (аллоцирует свой), потом
    /// вернёт его. Внешний вызов не увидит внутренний буфер и вернёт
    /// свой — никаких гонок, никакого UB.
    ///
    /// Все ранние `return` возвращают скретч обратно, чтобы следующая
    /// итерация не потеряла capacity.
    pub fn for_each_pair<A, B, F>(&mut self, mut f: F)
    where
        A: Send + Sync + 'static,
        B: Send + Sync + 'static,
        F: FnMut(Entity, &mut A, &B),
    {
        let a_type = TypeId::of::<A>();
        let b_type = TypeId::of::<B>();
        assert_ne!(a_type, b_type, "for_each_pair requires distinct component types");

        // ИЗМЕНЕНО (#9): `mem::take` — вынимаем скретч локально.
        // `self.scratch_entities` остаётся пустым `Vec::new()` на время
        // работы метода; вернём заполненный в конце.
        let mut entities = std::mem::take(&mut self.scratch_entities);
        entities.clear();

        // Заполняем буфер entity из A-стораджа без промежуточной
        // аллокации (`entities_iter()` возвращает `slice::Iter`,
        // `.copied()` даёт `u32`, `extend` пишет прямо в буфер).
        if let Some(s) = self.storages.get(&a_type) {
            if let Some(storage) = s.as_any().downcast_ref::<ComponentStorage<A>>() {
                entities.extend(storage.entities_iter().copied());
            }
        }

        if entities.is_empty() {
            // A-сторадж пуст или отсутствует — работа сделана,
            // возвращаем capacity-буфер в World.
            self.scratch_entities = entities;
            return;
        }

        // Raw pointers — обходим borrow checker, т.к. оба стораджа
        // берутся из одной HashMap. Разные TypeId → разные Box →
        // непересекающиеся регионы памяти.
        let a_ptr: *mut ComponentStorage<A> = self
            .storages
            .get_mut(&a_type)
            .and_then(|s| s.as_any_mut().downcast_mut::<ComponentStorage<A>>())
            .map(|s| s as *mut _)
            .unwrap();

        let b_ptr: *const ComponentStorage<B> = match self.storages.get(&b_type) {
            Some(s) => s
                .as_any()
                .downcast_ref::<ComponentStorage<B>>()
                .map(|s| s as *const _)
                .unwrap(),
            None => {
                // B-стораджа нет — пересечения быть не может.
                // Возвращаем скретч обратно в World.
                self.scratch_entities = entities;
                return;
            }
        };

        // SAFETY: два разных TypeId → две разные Box-аллокации → два
        // непересекающихся участка памяти. HashMap (`self.storages`)
        // не модифицируется во время цикла. `a_ptr` мутабельный, но
        // `b_ptr` указывает на другой объект.
        unsafe {
            let a_store = &mut *a_ptr;
            let b_store = &*b_ptr;
            for &e in &entities {
                if let (Some(a), Some(b)) = (a_store.get_mut(e), b_store.get(e)) {
                    f(e, a, b);
                }
            }
        }

        // ИЗМЕНЕНО (#9): возвращаем capacity-буфер в World.
        self.scratch_entities = entities;
    }

    // ============================================================
    // Events
    // ============================================================

    pub fn send<E: 'static + Send + Sync>(&mut self, event: E) {
        self.events.send(event);
    }

    pub fn read_events<E: 'static + Send + Sync>(&self) -> impl Iterator<Item = &E> {
        self.events.read::<E>()
    }

    pub fn read_events_current<E: 'static + Send + Sync>(&self) -> impl Iterator<Item = &E> {
        self.events.read_current::<E>()
    }

    pub fn update_events(&mut self) {
        self.events.update();
    }

    // ============================================================
    // Прочее
    // ============================================================

    pub fn entities(&self) -> &[Entity] { &self.entities }
    pub fn len(&self) -> usize { self.entities.len() }
    pub fn is_empty(&self) -> bool { self.entities.is_empty() }
}