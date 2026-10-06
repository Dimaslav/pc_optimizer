//! Настройки редактора. Сохраняются в `editor.ron` при выходе
//! и загружаются при старте.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::render::PostFx;

use super::camera_bookmarks::CameraBookmarks;

const MAX_RECENT: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSettings {
    // === Панели ===
    pub show_renderer_panel: bool,
    pub show_stats_panel: bool,
    pub show_hierarchy_panel: bool,
    pub show_inspector_panel: bool,
    /// ИЗМЕНЕНО (Фаза 4.4): видимость окна аудио.
    #[serde(default)]
    pub show_audio_panel: bool,
    pub left_panel_width: f32,
    pub right_panel_width: f32,

    // === Пути ===
    pub save_path: String,
    pub prefabs_dir: String,
    pub fbx_export_path: String,

    // === Fly ===
    pub fly_speed: f32,
    pub fly_sensitivity: f32,

    // === Gizmo / палитра ===
    pub gizmo_snap: bool,
    pub palette_keep_active: bool,
    pub palette_snap_to_grid: bool,
    pub palette_grid_step: f32,

    // === Recent ===
    pub recent_scenes: Vec<String>,

    /// Закладки камеры (9 слотов).
    #[serde(default)]
    pub camera_bookmarks: CameraBookmarks,

    /// Графические настройки (PostFx).
    #[serde(default)]
    pub postfx: PostFx,

    /// ИЗМЕНЕНО (Фаза 4.4): громкость аудио-шин по имени.
    ///
    /// Ключ — строковое имя шины (`"Master"`, `"Sfx"`, `"Music"`,
    /// `"Voice"`, `"Ui"`). Значение — громкость в `[0, 2]`.
    ///
    /// Строки, а не `AudioBus` — чтобы `editor.ron` оставался читаемым
    /// и не ломался при переименовании enum-вариантов. При загрузке
    /// `App` маппит имена обратно в `AudioBus`.
    #[serde(default)]
    pub audio_bus_volumes: HashMap<String, f32>,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            show_renderer_panel: true,
            show_stats_panel: true,
            show_hierarchy_panel: true,
            show_inspector_panel: true,
            show_audio_panel: false,
            left_panel_width: 260.0,
            right_panel_width: 340.0,
            save_path: "scene.ron".to_string(),
            prefabs_dir: "prefabs".to_string(),
            fbx_export_path: "scene.fbx".to_string(),
            fly_speed: 15.0,
            fly_sensitivity: 0.0025,
            gizmo_snap: false,
            palette_keep_active: true,
            palette_snap_to_grid: true,
            palette_grid_step: 0.5,
            recent_scenes: Vec::new(),
            camera_bookmarks: CameraBookmarks::new(),
            postfx: PostFx::default(),
            audio_bus_volumes: HashMap::new(),
        }
    }
}

impl EditorSettings {

    pub fn load(path: impl AsRef<Path>) -> Self {
        let path = path.as_ref();
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        match ron::from_str::<Self>(&text) {
            Ok(s) => {
                log::info!("Editor settings loaded from {}", path.display());
                s
            }
            Err(e) => {
                log::error!(
                    "Failed to parse editor settings '{}': {}. Using defaults.",
                    path.display(),
                    e
                );
                let backup = path.with_extension("ron.bak");
                match std::fs::rename(path, &backup) {
                    Ok(()) => log::warn!(
                        "Corrupt editor settings backed up to {}",
                        backup.display()
                    ),
                    Err(io_err) => log::warn!(
                        "Could not back up corrupt settings to {}: {}",
                        backup.display(),
                        io_err
                    ),
                }
                Self::default()
            }
        }
    }

    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        std::fs::write(path, text).map_err(|e| {
            log::warn!(
                "Failed to write editor settings to {}: {}",
                path.display(),
                e
            );
            e
        })
    }

    pub fn push_recent_scene(&mut self, path: &str) {
        self.recent_scenes.retain(|p| p != path);
        self.recent_scenes.insert(0, path.to_string());
        self.recent_scenes.truncate(MAX_RECENT);
    }
}