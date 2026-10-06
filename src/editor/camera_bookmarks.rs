//! Закладки камеры: сохранение позиций и переход к ним.
//! Хоткеи: Ctrl+1..9 — сохранить, Alt+1..9 — перейти.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CameraBookmark {
    pub target: [f32; 3],
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CameraBookmarks {
    /// 9 слотов, `None` — пустой.
    pub slots: [Option<CameraBookmark>; 9],
}

impl CameraBookmarks {
    pub fn new() -> Self {
        Self { slots: [None; 9] }
    }

    pub fn save(&mut self, slot: usize, bm: CameraBookmark) {
        if slot < 9 {
            self.slots[slot] = Some(bm);
        }
    }

    pub fn get(&self, slot: usize) -> Option<CameraBookmark> {
        if slot < 9 {
            self.slots[slot]
        } else {
            None
        }
    }

    pub fn is_empty(&self, slot: usize) -> bool {
        slot >= 9 || self.slots[slot].is_none()
    }

    pub fn iter(&self) -> impl Iterator<Item = (usize, CameraBookmark)> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.map(|bm| (i, bm)))
    }

    pub fn any(&self) -> bool {
        self.slots.iter().any(|s| s.is_some())
    }
}