//! Event bus с двойной буферизацией.
//!
//! В начале кадра события из `current` переезжают в `previous`.
//! Читатель видит события этого и предыдущего кадра.
//!
//! `update()` использует `swap` вместо `mem::take`, чтобы сохранить
//! аллокации HashMap и Vec — иначе каждый кадр создаётся новый HashMap.

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Функция очистки `Vec<E>` внутри `dyn Any` с сохранением capacity.
type ClearFn = fn(&mut dyn Any);

fn make_clear<E: 'static + Send + Sync>() -> ClearFn {
    fn clear<E: 'static>(a: &mut dyn Any) {
        if let Some(v) = a.downcast_mut::<Vec<E>>() {
            v.clear();
        }
    }
    clear::<E>
}

#[derive(Default)]
pub struct Events {
    current: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    previous: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
    /// TypeId → функция очистки. Заполняется при первом `send::<E>`.
    clearers: HashMap<TypeId, ClearFn>,
}

impl Events {
    pub fn new() -> Self { Self::default() }

    pub fn send<E: 'static + Send + Sync>(&mut self, event: E) {
        let tid = TypeId::of::<E>();
        self.clearers.entry(tid).or_insert_with(make_clear::<E>);

        let queue = self
            .current
            .entry(tid)
            .or_insert_with(|| Box::new(Vec::<E>::new()));
        queue.downcast_mut::<Vec<E>>().unwrap().push(event);
    }

    pub fn read<E: 'static + Send + Sync>(&self) -> impl Iterator<Item = &E> {
        let current = self
            .current
            .get(&TypeId::of::<E>())
            .and_then(|q| q.downcast_ref::<Vec<E>>())
            .map(|v| v.iter())
            .into_iter()
            .flatten();

        let previous = self
            .previous
            .get(&TypeId::of::<E>())
            .and_then(|q| q.downcast_ref::<Vec<E>>())
            .map(|v| v.iter())
            .into_iter()
            .flatten();

        current.chain(previous)
    }

    pub fn read_current<E: 'static + Send + Sync>(&self) -> impl Iterator<Item = &E> {
        self.current
            .get(&TypeId::of::<E>())
            .and_then(|q| q.downcast_ref::<Vec<E>>())
            .map(|v| v.iter())
            .into_iter()
            .flatten()
    }

    /// Вызывается движком в начале кадра: `current` → `previous`.
    ///
    /// Меняем местами контейнеры (сохраняя HashMap-аллокации),
    /// а затем очищаем Vec'ы (сохраняя их capacity).
    pub fn update(&mut self) {
        std::mem::swap(&mut self.current, &mut self.previous);
        for (tid, q) in self.current.iter_mut() {
            if let Some(clear) = self.clearers.get(tid) {
                clear(q.as_mut());
            }
        }
    }
}