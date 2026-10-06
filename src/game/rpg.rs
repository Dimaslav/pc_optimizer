//! RPG-сцена: деревня, лес, подземелье.
//!
//! Демонстрация игрового слоя на текущих возможностях движка:
//! - NPC с диалогами
//! - Квесты (убить N врагов)
//! - Сбор золота и ключей
//! - Сундуки, двери, порталы
//! - Босс

use std::collections::{HashMap, HashSet};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::ecs::{Entity, World};
use crate::game::components::*;
use crate::physics::{Collider, PhysicsMaterial, RigidBody};
use crate::render::{AlphaMode, Material, Renderer};

// ============================================================
// Компоненты
// ============================================================

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GoldValue(pub u32);

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct KeyItem(pub u32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chest {
    pub gold: u32,
    pub opened: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Door {
    pub needs_key: u32,
    pub open: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Npc {
    pub name: String,
    pub lines: Vec<String>,
    pub quest_id: Option<u32>,
    pub spoken_to: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct QuestTarget {
    pub quest_id: u32,
}

// ============================================================
// Состояние
// ============================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quest {
    pub title: String,
    pub total: u32,
    pub current: u32,
    pub done: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RpgState {
    pub gold: u32,
    pub keys: Vec<u32>,
    pub quests: HashMap<u32, Quest>,
    pub active_quest: Option<u32>,
    /// (speaker, text, time_left_sec)
    pub dialogue: Option<(String, String, f32)>,
    claimed: HashSet<u32>,
}

impl RpgState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_dialogue(&mut self, speaker: &str, text: &str) {
        self.dialogue = Some((speaker.to_string(), text.to_string(), 5.0));
    }

    pub fn tick_dialogue(&mut self, dt: f32) {
        if let Some((_, _, t)) = &mut self.dialogue {
            *t -= dt;
            if *t <= 0.0 {
                self.dialogue = None;
            }
        }
    }

    pub fn add_gold(&mut self, n: u32) {
        self.gold += n;
    }

    pub fn has_key(&self, id: u32) -> bool {
        self.keys.contains(&id)
    }

    pub fn give_key(&mut self, id: u32) {
        if !self.has_key(id) {
            self.keys.push(id);
        }
    }

    pub fn start_quest(&mut self, id: u32, title: &str, total: u32) {
        if !self.quests.contains_key(&id) {
            self.quests.insert(id, Quest {
                title: title.to_string(),
                total,
                current: 0,
                done: false,
            });
            self.active_quest = Some(id);
        }
    }

    pub fn progress_quest(&mut self, id: u32) -> bool {
        let Some(q) = self.quests.get_mut(&id) else { return false; };
        if q.done { return false; }
        q.current = (q.current + 1).min(q.total);
        if q.current >= q.total {
            q.done = true;
            return true;
        }
        false
    }

    pub fn is_quest_done(&self, id: u32) -> bool {
        self.quests.get(&id).map(|q| q.done).unwrap_or(false)
    }

    pub fn has_claimed(&self, id: u32) -> bool {
        self.claimed.contains(&id)
    }

    pub fn mark_claimed(&mut self, id: u32) {
        self.claimed.insert(id);
    }

    pub fn active_quest_summary(&self) -> Option<(String, u32, u32, bool)> {
        let id = self.active_quest?;
        let q = self.quests.get(&id)?;
        Some((q.title.clone(), q.current, q.total, q.done))
    }

    pub fn hud_lines(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        out.push(("💰 Gold".into(), format!("{}", self.gold)));
        if !self.keys.is_empty() {
            let ks: Vec<String> = self.keys.iter().map(|k| k.to_string()).collect();
            out.push(("🔑 Keys".into(), ks.join(", ")));
        }
        if let Some((title, cur, total, done)) = self.active_quest_summary() {
            let mark = if done { "✓" } else { "…" };
            out.push((
                format!("📜 {} {}", mark, title),
                format!("{}/{}", cur, total),
            ));
        }
        if let Some((speaker, text, _)) = &self.dialogue {
            out.push((format!("💬 {}", speaker), text.clone()));
        }
        out
    }
}

// ============================================================
// Материалы
// ============================================================

pub fn register_materials(renderer: &mut Renderer) {
    renderer.add_material("rpg_roof",
        Material::new([0.4, 0.15, 0.1, 1.0]).with_metallic_roughness(0.0, 0.8));
    renderer.add_material("rpg_wall",
        Material::new([0.8, 0.75, 0.65, 1.0]).with_metallic_roughness(0.0, 0.9));
    renderer.add_material("rpg_dungeon",
        Material::new([0.13, 0.11, 0.16, 1.0]).with_metallic_roughness(0.0, 0.95));
    renderer.add_material("rpg_coin",
        Material::new([1.0, 0.85, 0.2, 1.0]).with_metallic_roughness(1.0, 0.3));
    renderer.add_material("rpg_key",
        Material::new([1.0, 0.9, 0.5, 1.0])
            .with_metallic_roughness(0.8, 0.3)
            .with_emissive([1.5, 1.2, 0.3]));
    renderer.add_material("rpg_npc",
        Material::new([0.9, 0.75, 0.6, 1.0]).with_metallic_roughness(0.0, 0.7));
    renderer.add_material("rpg_quest",
        Material::new([1.0, 1.0, 0.4, 1.0])
            .with_metallic_roughness(0.0, 0.5)
            .with_emissive([2.0, 2.0, 0.5]));
    renderer.add_material("rpg_wolf",
        Material::new([0.35, 0.3, 0.28, 1.0]).with_metallic_roughness(0.0, 0.8));
    renderer.add_material("rpg_boss",
        Material::new([0.7, 0.1, 0.1, 1.0])
            .with_metallic_roughness(0.2, 0.5)
            .with_emissive([0.8, 0.1, 0.1]));
    renderer.add_material("rpg_portal",
        Material::new([0.6, 0.3, 1.0, 0.5])
            .with_metallic_roughness(0.0, 0.1)
            .with_emissive([0.8, 0.3, 2.0])
            .with_alpha_mode(AlphaMode::Blend));
    renderer.add_material("rpg_chest",
        Material::new([0.5, 0.3, 0.1, 1.0]).with_metallic_roughness(0.2, 0.6));
    renderer.add_material("rpg_door",
        Material::new([0.55, 0.35, 0.15, 1.0]).with_metallic_roughness(0.1, 0.7));
}

// ============================================================
// Спавн сцены
// ============================================================

pub fn spawn_scene(world: &mut World) {
    spawn_ground(world);
    spawn_village(world);
    spawn_npcs(world);
    spawn_forest(world);
    spawn_dungeon(world);
    spawn_center_marker(world);
}

// ---------- Ground ----------

fn spawn_ground(world: &mut World) {
    let g = world.spawn();
    world.insert(g, Name("Ground".into()));
    world.insert(g, Transform::new(0.0, 0.0, 0.0));
    world.insert(g, MeshHandle("ground".into()));
    world.insert(g, MaterialHandle("ground".into()));
    world.insert(g, RigidBody::static_body());
    world.insert(g, Collider::aabb(Vec3::new(80.0, 0.01, 80.0)));
    world.insert(g, PhysicsMaterial::concrete());
}

// ---------- Village ----------

fn spawn_village(world: &mut World) {
    for i in 0..6 {
        let angle = i as f32 / 6.0 * std::f32::consts::TAU;
        let r = 12.0;
        let hx = angle.cos() * r;
        let hz = angle.sin() * r;

        let house = world.spawn();
        world.insert(house, Name(format!("House_{}", i)));
        world.insert(house, Transform::new(hx, 1.5, hz)
            .with_rotation(glam::Quat::from_axis_angle(Vec3::Y, -angle))
            .with_scale_xyz(4.0, 3.0, 4.0));
        world.insert(house, MeshHandle("cube".into()));
        world.insert(house, MaterialHandle("rpg_wall".into()));
        world.insert(house, RigidBody::static_body());
        world.insert(house, Collider::aabb(Vec3::splat(0.5)));
        world.insert(house, PhysicsMaterial::concrete());

        let roof = world.spawn();
        world.insert(roof, Name(format!("Roof_{}", i)));
        world.insert(roof, Transform::new(hx, 3.3, hz)
            .with_rotation(glam::Quat::from_axis_angle(Vec3::Y, -angle))
            .with_scale_xyz(4.4, 0.6, 4.4));
        world.insert(roof, MeshHandle("cube".into()));
        world.insert(roof, MaterialHandle("rpg_roof".into()));
        world.insert(roof, RigidBody::static_body());
        world.insert(roof, Collider::aabb(Vec3::splat(0.5)));
    }

    // Колодец
    let well = world.spawn();
    world.insert(well, Name("Well".into()));
    world.insert(well, Transform::new(0.0, 0.6, 0.0).with_scale_xyz(1.5, 1.2, 1.5));
    world.insert(well, MeshHandle("cylinder".into()));
    world.insert(well, MaterialHandle("rpg_wall".into()));
    world.insert(well, RigidBody::static_body());
    world.insert(well, Collider::aabb(Vec3::splat(0.5)));

    // Сундук рядом с колодцем
    spawn_chest(world, Vec3::new(2.5, 0.4, 0.5), 50);
}

fn spawn_chest(world: &mut World, pos: Vec3, gold: u32) {
    let e = world.spawn();
    world.insert(e, Name("Chest".into()));
    world.insert(e, Transform::at(pos).with_scale_xyz(0.8, 0.5, 0.6));
    world.insert(e, MeshHandle("cube".into()));
    world.insert(e, MaterialHandle("rpg_chest".into()));
    world.insert(e, RigidBody::static_body());
    world.insert(e, Collider::aabb(Vec3::splat(0.5)));
    world.insert(e, Chest { gold, opened: false });
    world.insert(e, Spinner::new(Vec3::Y, 0.3));
}

// ---------- NPCs ----------

fn spawn_npcs(world: &mut World) {
    spawn_npc(world, Vec3::new(-3.0, 1.0, 3.0), "Elder", vec![
        "Приветствую, путник.",
        "В лесах развелись волки. Убей 5 — и награда твоя.",
        "Возвращайся ко мне, когда закончишь.",
    ], Some(1));

    spawn_npc(world, Vec3::new(3.0, 1.0, 3.0), "Merchant", vec![
        "Золото — универсальный язык.",
        "Ищи монеты по лесу и в подземелье.",
        "Ты уже собрал что-нибудь?",
    ], None);

    spawn_npc(world, Vec3::new(0.0, 1.0, -5.0), "Blacksmith", vec![
        "Ключ от подземелья спрятан у босса.",
        "Порталом можно быстро туда добраться.",
        "Осторожнее — там темно и опасно.",
    ], None);
}

fn spawn_npc(
    world: &mut World,
    pos: Vec3,
    name: &str,
    lines: Vec<&str>,
    quest_id: Option<u32>,
) {
    let e = world.spawn();
    world.insert(e, Name(format!("NPC_{}", name)));
    world.insert(e, Transform::at(pos).with_scale_xyz(0.6, 1.8, 0.6));
    world.insert(e, MeshHandle("capsule".into()));
    world.insert(e, MaterialHandle("rpg_npc".into()));
    world.insert(e, RigidBody::static_body());
    world.insert(e, Collider::capsule(0.4, 1.8));
    world.insert(e, Npc {
        name: name.to_string(),
        lines: lines.iter().map(|s| s.to_string()).collect(),
        quest_id,
        spoken_to: 0,
    });
}

// ---------- Forest ----------

fn spawn_forest(world: &mut World) {
    for i in 0..40 {
        let angle = i as f32 / 40.0 * std::f32::consts::TAU;
        let r = 20.0 + ((i as f32 * 0.7).sin() + 1.0) * 7.0;
        let x = angle.cos() * r;
        let z = angle.sin() * r;
        let e = world.spawn();
        world.insert(e, Name(format!("Tree_{}", i)));
        world.insert(e, Transform::new(x, 3.0, z)
            .with_rotation(glam::Quat::from_axis_angle(Vec3::Y, angle))
            .with_scale_xyz(3.0, 6.0, 1.0));
        world.insert(e, MeshHandle("quad_xy".into()));
        world.insert(e, MaterialHandle("foliage".into()));
        world.insert(e, RigidBody::static_body());
        world.insert(e, Collider::capsule(0.5, 6.0));
    }

    for i in 0..10 {
        let angle = i as f32 / 10.0 * std::f32::consts::TAU + 0.15;
        let r = 22.0 + (i as f32 * 1.3).sin() * 3.0;
        let x = angle.cos() * r;
        let z = angle.sin() * r;
        let e = world.spawn();
        world.insert(e, Name(format!("Wolf_{}", i)));
        world.insert(e, Transform::new(x, 0.5, z).with_scale(0.6));
        world.insert(e, MeshHandle("sphere".into()));
        world.insert(e, MaterialHandle("rpg_wolf".into()));
        world.insert(e, Health::new(30.0));
        world.insert(e, Chase::new(4.0, 1.2));
        world.insert(e, QuestTarget { quest_id: 1 });
    }

    for i in 0..20 {
        let angle = i as f32 / 20.0 * std::f32::consts::TAU + 0.3;
        let r = 22.0 + (i as f32 * 0.9).cos() * 6.0;
        spawn_coin(world, Vec3::new(angle.cos() * r, 0.3, angle.sin() * r), 5);
    }
}

// ---------- Dungeon ----------

fn spawn_dungeon(world: &mut World) {
    let d = Vec3::new(-50.0, 0.0, 0.0);

    let floor = world.spawn();
    world.insert(floor, Name("DungeonFloor".into()));
    world.insert(floor, Transform::at(d + Vec3::Y * 0.05)
        .with_scale_xyz(30.0, 0.1, 30.0));
    world.insert(floor, MeshHandle("cube".into()));
    world.insert(floor, MaterialHandle("rpg_dungeon".into()));
    world.insert(floor, RigidBody::static_body());
    world.insert(floor, Collider::aabb(Vec3::splat(0.5)));

    let wall_specs = [
        (d + Vec3::new(0.0, 3.0, 15.0), Vec3::new(30.0, 6.0, 0.5)),
        (d + Vec3::new(0.0, 3.0, -15.0), Vec3::new(30.0, 6.0, 0.5)),
        (d + Vec3::new(15.0, 3.0, 0.0), Vec3::new(0.5, 6.0, 30.0)),
        (d + Vec3::new(-15.0, 3.0, 0.0), Vec3::new(0.5, 6.0, 30.0)),
    ];
    for (i, (pos, size)) in wall_specs.into_iter().enumerate() {
        let e = world.spawn();
        world.insert(e, Name(format!("DungeonWall_{}", i)));
        world.insert(e, Transform::at(pos).with_scale_xyz(size.x, size.y, size.z));
        world.insert(e, MeshHandle("cube".into()));
        world.insert(e, MaterialHandle("rpg_dungeon".into()));
        world.insert(e, RigidBody::static_body());
        world.insert(e, Collider::aabb(Vec3::splat(0.5)));
    }

    let door = world.spawn();
    world.insert(door, Name("DungeonDoor".into()));
    world.insert(door, Transform::new(d.x, 1.5, d.z + 15.0)
        .with_scale_xyz(2.0, 3.0, 0.3));
    world.insert(door, MeshHandle("cube".into()));
    world.insert(door, MaterialHandle("rpg_door".into()));
    world.insert(door, RigidBody::static_body());
    world.insert(door, Collider::aabb(Vec3::splat(0.5)));
    world.insert(door, Door { needs_key: 1, open: false });

    spawn_key(world, d + Vec3::new(0.0, 0.3, -10.0), 1);
    spawn_chest(world, d + Vec3::new(5.0, 0.4, -10.0), 500);

    let boss = world.spawn();
    world.insert(boss, Name("Boss".into()));
    world.insert(boss, Transform::at(d + Vec3::new(0.0, 1.5, -5.0)).with_scale(2.0));
    world.insert(boss, MeshHandle("sphere".into()));
    world.insert(boss, MaterialHandle("rpg_boss".into()));
    world.insert(boss, Health::new(300.0));
    world.insert(boss, Chase::new(2.0, 2.0));
    world.insert(boss, QuestTarget { quest_id: 2 });

    for i in 0..4 {
        let angle = i as f32 / 4.0 * std::f32::consts::TAU;
        let pos = d + Vec3::new(angle.cos() * 8.0, 0.5, angle.sin() * 8.0 - 5.0);
        let e = world.spawn();
        world.insert(e, Name(format!("DungeonWolf_{}", i)));
        world.insert(e, Transform::at(pos).with_scale(0.7));
        world.insert(e, MeshHandle("sphere".into()));
        world.insert(e, MaterialHandle("rpg_wolf".into()));
        world.insert(e, Health::new(60.0));
        world.insert(e, Chase::new(4.5, 1.0));
        world.insert(e, QuestTarget { quest_id: 1 });
    }

    spawn_portal(
        world,
        Vec3::new(0.0, 1.0, 18.0),
        d + Vec3::new(0.0, 1.0, 8.0),
    );
    spawn_portal(
        world,
        d + Vec3::new(0.0, 1.0, 13.5),
        Vec3::new(0.0, 1.0, 22.0),
    );
}

fn spawn_portal(world: &mut World, from: Vec3, to: Vec3) {
    let e = world.spawn();
    world.insert(e, Name("Portal".into()));
    world.insert(e, Transform::at(from).with_scale_xyz(2.0, 4.0, 2.0));
    world.insert(e, MeshHandle("cube".into()));
    world.insert(e, MaterialHandle("rpg_portal".into()));
    let mut trig = Trigger::new(2.0, TriggerAction::Teleport(to.to_array()));
    trig.once = false;
    world.insert(e, trig);
    world.insert(e, Spinner::new(Vec3::Y, 0.8));
}

fn spawn_coin(world: &mut World, pos: Vec3, value: u32) {
    let e = world.spawn();
    world.insert(e, Name("Coin".into()));
    world.insert(e, Transform::at(pos).with_scale(0.3));
    world.insert(e, MeshHandle("cylinder".into()));
    world.insert(e, MaterialHandle("rpg_coin".into()));
    world.insert(e, GoldValue(value));
    world.insert(e, Spinner::new(Vec3::Y, 2.0));
}

fn spawn_key(world: &mut World, pos: Vec3, id: u32) {
    let e = world.spawn();
    world.insert(e, Name("Key".into()));
    world.insert(e, Transform::at(pos).with_scale(0.4));
    world.insert(e, MeshHandle("cube".into()));
    world.insert(e, MaterialHandle("rpg_key".into()));
    world.insert(e, KeyItem(id));
    world.insert(e, Spinner::new(Vec3::new(0.2, 1.0, 0.3), 3.0));
}

fn spawn_center_marker(world: &mut World) {
    let e = world.spawn();
    world.insert(e, Name("CenterMarker".into()));
    world.insert(e, Transform::new(0.0, 5.0, 0.0).with_scale(0.5));
    world.insert(e, MeshHandle("sphere".into()));
    world.insert(e, MaterialHandle("rpg_quest".into()));
    world.insert(e, Spinner::new(Vec3::Y, 1.5));
}

// ============================================================
// Системы
// ============================================================

pub fn tick(state: &mut RpgState, world: &mut World, player_pos: Vec3, dt: f32) {
    state.tick_dialogue(dt);

    let doors: Vec<Entity> = world
        .entities()
        .iter()
        .copied()
        .filter(|&e| world.has::<Door>(e))
        .collect();
    for e in doors {
        let needs_key = match world.get::<Door>(e) {
            Some(d) if !d.open => d.needs_key,
            _ => continue,
        };
        let pos = match world.get::<Transform>(e) {
            Some(t) => t.position,
            None => continue,
        };
        if (player_pos - pos).length() < 3.0 && state.has_key(needs_key) {
            if let Some(t) = world.get_mut::<Transform>(e) {
                t.scale.y = 0.05;
                t.position.y = 0.1;
            }
            world.insert(e, MaterialHandle("rpg_portal".into()));
            if let Some(d) = world.get_mut::<Door>(e) {
                d.open = true;
            }
            state.push_dialogue("Дверь", "Ключ подошёл — дверь открылась.");
        }
    }
}

pub fn try_interact(world: &mut World, state: &mut RpgState, target: Entity) -> bool {
    if let Some(npc) = world.get::<Npc>(target).cloned() {
        if let Some(qid) = npc.quest_id {
            if npc.spoken_to == 0 {
                state.start_quest(qid, "Убить 5 волков", 5);
                state.push_dialogue(&npc.name, "Найди и убей 5 волков в лесу.");
            } else if state.is_quest_done(qid) && !state.has_claimed(qid) {
                state.mark_claimed(qid);
                state.add_gold(200);
                state.push_dialogue(&npc.name, "Отличная работа! Возьми 200 монет.");
            } else {
                let idx = (npc.spoken_to as usize) % npc.lines.len();
                let line = npc.lines.get(idx).cloned().unwrap_or_default();
                state.push_dialogue(&npc.name, &line);
            }
        } else {
            let idx = (npc.spoken_to as usize) % npc.lines.len();
            let line = npc.lines.get(idx).cloned().unwrap_or_default();
            state.push_dialogue(&npc.name, &line);
        }
        if let Some(n) = world.get_mut::<Npc>(target) {
            n.spoken_to += 1;
        }
        return true;
    }

    if let Some(g) = world.get::<GoldValue>(target).copied() {
        state.add_gold(g.0);
        state.push_dialogue("💰", &format!("+{} монет", g.0));
        world.despawn(target);
        return true;
    }

    if let Some(k) = world.get::<KeyItem>(target).copied() {
        state.give_key(k.0);
        state.push_dialogue("🔑", "Ты подобрал ключ.");
        world.despawn(target);
        return true;
    }

    if let Some(c) = world.get::<Chest>(target).cloned() {
        if c.opened {
            state.push_dialogue("📦", "Уже пусто.");
            return true;
        }
        state.add_gold(c.gold);
        state.push_dialogue("📦", &format!("Ты нашёл {} монет!", c.gold));
        if let Some(ch) = world.get_mut::<Chest>(target) {
            ch.opened = true;
        }
        world.insert(target, MaterialHandle("rpg_portal".into()));
        return true;
    }

    false
}

pub fn on_kill(world: &mut World, state: &mut RpgState, target: Entity) {
    let qid = match world.get::<QuestTarget>(target) {
        Some(q) => q.quest_id,
        None => return,
    };

    let done = state.progress_quest(qid);
    let pos = world
        .get::<Transform>(target)
        .map(|t| t.position)
        .unwrap_or(Vec3::ZERO);

    if done {
        state.push_dialogue("✓ Квест выполнен!", "Вернись к Elder за наградой.");
    } else {
        let cur = state.quests.get(&qid).map(|q| q.current).unwrap_or(0);
        state.push_dialogue("Враг убит", &format!("Прогресс: {}/5", cur));
    }

    let drop = world.spawn();
    world.insert(drop, Name("CoinDrop".into()));
    world.insert(drop, Transform::at(pos).with_scale(0.3));
    world.insert(drop, MeshHandle("cylinder".into()));
    world.insert(drop, MaterialHandle("rpg_coin".into()));
    world.insert(drop, GoldValue(3));
    world.insert(drop, Spinner::new(Vec3::Y, 2.0));
}