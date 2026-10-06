//! Префабы: сохранение группы entity в файл `.prefab.ron`,
//! спавн инстансов из файла.
//!
//! `Parent` внутри префаба кодируется **индексом в массиве**, а не real
//! entity id. При `instantiate_prefab` индексы конвертируются в новые
//! entity id второго прохода.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Vec3;

use crate::ecs::{Entity, World};
use crate::game::components::Parent;
use crate::scene::serialize::{snapshot_entity, spawn_snapshot, EntitySnapshot};

/// Формат файла `.prefab.ron`.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct PrefabFile {
    #[serde(default)]
    pub name: Option<String>,
    pub entities: Vec<EntitySnapshot>,
}

/// Построить префаб из выделенных entity.
///
/// `Parent` для ссылок внутри выделения перекодируется в индекс в массиве
/// (0..N-1). Ссылки наружу — отбрасываются (`None`).
pub fn prefab_from_selection(
    world: &World,
    selected: &[Entity],
    name: Option<String>,
) -> PrefabFile {
    let mut sorted: Vec<Entity> = selected.to_vec();
    sorted.sort();
    sorted.dedup();

    let id_to_idx: HashMap<Entity, u32> = sorted
        .iter()
        .enumerate()
        .map(|(i, &e)| (e, i as u32))
        .collect();

    let mut entities: Vec<EntitySnapshot> = Vec::with_capacity(sorted.len());
    for &e in &sorted {
        let Some(mut snap) = snapshot_entity(world, e) else {
            continue;
        };
        // Перекодируем Parent: real entity → index в префабе.
        // Ссылки наружу префаба → None.
        if let Some(p) = snap.parent {
            snap.parent = id_to_idx.get(&p).copied();
        }
        entities.push(snap);
    }

    PrefabFile { name, entities }
}

/// Сохранить префаб в файл.
pub fn save_prefab_to_file(prefab: &PrefabFile, path: impl AsRef<Path>) -> Result<()> {
    let text = ron::ser::to_string_pretty(prefab, ron::ser::PrettyConfig::default())
        .context("serialize prefab")?;
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(path, text)
        .with_context(|| format!("write prefab {}", path.display()))?;
    Ok(())
}

/// Загрузить префаб из файла.
pub fn load_prefab_from_file(path: impl AsRef<Path>) -> Result<PrefabFile> {
    let text = std::fs::read_to_string(path.as_ref())
        .with_context(|| format!("read prefab {}", path.as_ref().display()))?;
    ron::from_str(&text).context("parse RON prefab")
}

/// Спавнит все entity из префаба. Возвращает список новых entity.
///
/// Смещение `offset` применяется **только к корневым** entity префаба
/// (у которых `parent == None` или указывает за пределы префаба).
/// Иначе дочерние entity получили бы offset дважды: один раз через
/// свой локальный Transform, второй — через родителя.
pub fn instantiate_prefab(
    world: &mut World,
    prefab: &PrefabFile,
    offset: Vec3,
) -> Vec<Entity> {
    let n = prefab.entities.len() as u32;

    let is_root = |parent: Option<u32>| -> bool {
        match parent {
            None => true,
            Some(p) => p >= n,
        }
    };

    let mut idx_to_entity: HashMap<u32, Entity> = HashMap::new();
    let mut new_entities: Vec<Entity> = Vec::with_capacity(prefab.entities.len());

    // Первый проход: spawn всех без Parent.
    for (idx, snap) in prefab.entities.iter().enumerate() {
        let mut s = snap.clone();
        s.parent = None;

        if is_root(snap.parent) {
            if let Some(ref mut t) = s.transform {
                t.position[0] += offset.x;
                t.position[1] += offset.y;
                t.position[2] += offset.z;
            }
        }

        let e = spawn_snapshot(world, s);
        idx_to_entity.insert(idx as u32, e);
        new_entities.push(e);
    }

    // Второй проход: восстановить Parent через маппинг индексов.
    for (idx, snap) in prefab.entities.iter().enumerate() {
        let Some(old_parent_idx) = snap.parent else { continue };
        // Ссылки наружу префаба отбрасываем.
        if old_parent_idx >= n {
            continue;
        }
        let (Some(&new_parent), Some(&new_self)) = (
            idx_to_entity.get(&old_parent_idx),
            idx_to_entity.get(&(idx as u32)),
        ) else {
            continue;
        };
        world.insert(new_self, Parent(new_parent));
    }

    new_entities
}

/// Список файлов `*.prefab.ron` в папке. Сортирован по имени.
pub fn list_prefabs(dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir.as_ref()) else {
        return out;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let Some(name) = p.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name.ends_with(".prefab.ron") {
            out.push(p);
        }
    }
    out.sort();
    out
}

/// Отображаемое имя файла префаба (без `.prefab.ron`).
pub fn prefab_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.trim_end_matches(".prefab.ron").to_string())
        .unwrap_or_else(|| "?".to_string())
}