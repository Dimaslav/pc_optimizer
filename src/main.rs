#![allow(dead_code)]
#![allow(unused_imports)]

mod ecs;
mod editor;
mod engine;
mod game;
mod physics;
mod render;
mod scene;

use std::any::Any;
use std::collections::HashMap;

use ecs::{Entity, System, World};
use engine::{run, Game, Input, InputMap, Key, MouseBtn};
use game::animation::{AnimationEvent, AnimationEvents, AnimationRuntime};
use game::audio::AudioSource;
use game::components::{
    AnimationPlayer, Elevator, ElevatorState, Interactable, MaterialHandle, MeshHandle, Name,
    Parent, SkeletonHandle, SlidingDoor, Spinner, TextureTiling, Tint, Transform, Trigger,
    TriggerAction, Velocity, Visible,
};
use game::decals::Decal;
use game::lights::{DirectionalLight, PointLight};
use game::rpg::{self, RpgState};
use game::timers::{Timer, TimerFinished, TimerSystem};
use game::AudioBus;
use glam::{Quat, Vec3};
use physics::{BodyType, Collider, RigidBody};
use render::{
    skinning::AnimationClip, AlphaMode, Camera3D, DebugView, GltfInstance, GpuLight,
    GpuPointLight, InstanceData, LineBatch, LineVertex, Material, Mesh, MeshDraw, PostFx,
    Renderer, Skeleton,
};
use winit::event::MouseButton;
use winit::keyboard::KeyCode;

fn sphere_in_frustum(center: Vec3, radius: f32, planes: &[glam::Vec4; 6]) -> bool {
    for p in planes {
        let dist = p.x * center.x + p.y * center.y + p.z * center.z + p.w;
        if dist < -radius { return false; }
    }
    true
}

fn compute_uv_scale(mesh: &Mesh, model: &glam::Mat4, tiling_size: f32) -> [f32; 2] {
    let local_extent = (mesh.aabb_max - mesh.aabb_min).abs();
    let (scale, _, _) = model.to_scale_rotation_translation();
    let scale_abs = scale.abs();
    let world_extent = local_extent * scale_abs;

    let tile = tiling_size.max(0.001);
    let mut uv_x = world_extent.x.max(world_extent.z) / tile;
    let mut uv_y = world_extent.y / tile;

    if uv_y < 0.001 { uv_y = uv_x; }
    if uv_x < 0.001 { uv_x = uv_y; }
    [uv_x, uv_y]
}

struct RotationSystem;
impl System for RotationSystem {
    fn update(&mut self, world: &mut World, dt: f32) {
        let entities: Vec<_> = world.query::<Spinner>().map(|(e, _)| e).collect();
        for e in entities {
            let spinner = world.get::<Spinner>(e).copied();
            if let (Some(s), Some(t)) = (spinner, world.get_mut::<Transform>(e)) {
                let axis = s.axis.normalize_or_zero();
                if axis.length_squared() < 1e-6 { continue; }
                let dq = Quat::from_axis_angle(axis, s.speed * dt);
                t.rotation = dq * t.rotation;
            }
        }
    }
}

struct MovementSystem;
impl System for MovementSystem {
    fn update(&mut self, world: &mut World, dt: f32) {
        world.for_each_pair::<Transform, Velocity, _>(|_e, t, v| {
            t.position += v.value * dt;
        });
    }
}

struct ElevatorSystem;
impl System for ElevatorSystem {
    fn update(&mut self, world: &mut World, dt: f32) {
        let entities: Vec<_> = world.query::<Elevator>().map(|(e, _)| e).collect();
        for e in entities {
            let Some(mut el) = world.get::<Elevator>(e).cloned() else { continue };
            let current_y = world
                .get::<Transform>(e)
                .map(|t| t.position.y)
                .unwrap_or(0.0);

            match el.state {
                ElevatorState::Idle => { el.current_velocity = 0.0; }
                ElevatorState::DoorsOpening => {
                    el.current_velocity = 0.0;
                    el.doors_open = (el.doors_open + el.door_speed * dt).min(1.0);
                    if el.doors_open >= 1.0 {
                        el.state = ElevatorState::DoorsOpen;
                        el.dwell_timer = el.dwell;
                    }
                }
                ElevatorState::DoorsOpen => {
                    el.current_velocity = 0.0;
                    if el.player_inside { el.dwell_timer = el.dwell; }
                    else {
                        el.dwell_timer -= dt;
                        if el.dwell_timer <= 0.0 { el.state = ElevatorState::DoorsClosing; }
                    }
                }
                ElevatorState::DoorsClosing => {
                    el.current_velocity = 0.0;
                    if el.player_inside { el.state = ElevatorState::DoorsOpening; }
                    else {
                        el.doors_open = (el.doors_open - el.door_speed * dt).max(0.0);
                        if el.doors_open <= 0.0 {
                            if el.target_floor != el.current_floor { el.state = ElevatorState::Moving; }
                            else { el.state = ElevatorState::Idle; }
                        }
                    }
                }
                ElevatorState::Moving => {
                    let arrived = el.update_moving(current_y, dt);
                    if arrived { el.state = ElevatorState::DoorsOpening; }
                }
            }

            if let Some(rb) = world.get_mut::<RigidBody>(e) {
                if rb.body_type == BodyType::Kinematic {
                    rb.velocity = Vec3::new(0.0, el.current_velocity, 0.0);
                }
            }

            let doors: Vec<Entity> = world.entities().iter().copied()
                .filter(|&d| {
                    world.get::<SlidingDoor>(d).is_some()
                        && world.get::<Parent>(d).map(|p| p.0 == e).unwrap_or(false)
                })
                .collect();
            for d in doors {
                if let Some(sd) = world.get_mut::<SlidingDoor>(d) {
                    sd.target = el.doors_open;
                }
            }

            if let Some(slot) = world.get_mut::<Elevator>(e) { *slot = el; }
        }
    }
}

struct SlidingDoorSystem;
impl System for SlidingDoorSystem {
    fn update(&mut self, world: &mut World, dt: f32) {
        let entities: Vec<_> = world.query::<SlidingDoor>().map(|(e, _)| e).collect();
        for e in entities {
            let Some(mut sd) = world.get::<SlidingDoor>(e).copied() else { continue };
            let Some(t) = world.get_mut::<Transform>(e) else { continue };

            let diff = sd.target - sd.open_amount;
            if diff.abs() < 1e-4 { sd.open_amount = sd.target; }
            else {
                let step = sd.speed * dt;
                if step >= diff.abs() { sd.open_amount = sd.target; }
                else { sd.open_amount += step * diff.signum(); }
            }

            t.position = sd.closed_position + sd.slide_axis * sd.slide_distance * sd.open_amount;
            if let Some(slot) = world.get_mut::<SlidingDoor>(e) { *slot = sd; }
        }
    }
}

fn spawn_elevator(world: &mut World, xz: (f32, f32), floors_y: Vec<f32>, speed: f32) {
    let y0 = *floors_y.first().unwrap_or(&0.0);
    let top_y = *floors_y.last().unwrap_or(&y0);
    let plat_scale = Vec3::new(3.0, 0.2, 3.0);

    let plat = world.spawn();
    world.insert(plat, Name("Elevator".into()));
    world.insert(plat, Transform::at(Vec3::new(xz.0, y0, xz.1))
        .with_scale_xyz(plat_scale.x, plat_scale.y, plat_scale.z));
    world.insert(plat, MeshHandle("cube".into()));
    world.insert(plat, MaterialHandle("flat_blue".into()));
    world.insert(plat, TextureTiling::new(1.5));
    world.insert(plat, RigidBody::kinematic());
    world.insert(plat, Collider::aabb(Vec3::splat(0.5)));

    let mut el = Elevator::new(floors_y.clone(), speed);
    el.acceleration = 3.0;
    el.sensor_radius = 2.0;
    world.insert(plat, el);

    for side in [-1.0_f32, 1.0] {
        let d = world.spawn();
        world.insert(d, Name(format!("ElevatorDoor_{}", if side < 0.0 { "L" } else { "R" })));

        let desired_world_size = Vec3::new(1.4, 2.0, 0.1);
        let local_scale = desired_world_size / plat_scale;

        let desired_world_pos = Vec3::new(side * 0.7, 1.0, 1.5);
        let local_pos = desired_world_pos / plat_scale;

        world.insert(d, Transform::at(local_pos)
            .with_scale_xyz(local_scale.x, local_scale.y, local_scale.z));
        world.insert(d, MeshHandle("cube".into()));
        world.insert(d, MaterialHandle("rpg_door".into()));
        world.insert(d, Parent(plat));
        world.insert(d, Collider::aabb(Vec3::splat(0.5)));

        let local_slide_distance = 1.4 / plat_scale.x;
        world.insert(d, SlidingDoor::new(local_pos, Vec3::X * side, local_slide_distance));
    }

    let shaft_height = (top_y - y0) + 6.0;
    let shaft_center_y = (y0 + top_y) * 0.5 + 1.0;

    let wall_specs = [
        (Vec3::new(0.0, 0.0, -1.75), Vec3::new(3.6, shaft_height, 0.2)),
        (Vec3::new(-1.75, 0.0, 0.0), Vec3::new(0.2, shaft_height, 3.6)),
        (Vec3::new(1.75, 0.0, 0.0), Vec3::new(0.2, shaft_height, 3.6)),
    ];

    for (i, (offset, size)) in wall_specs.iter().enumerate() {
        let w = world.spawn();
        world.insert(w, Name(format!("ElevatorShaft_{}", i)));
        world.insert(w, Transform::at(Vec3::new(xz.0 + offset.x, shaft_center_y, xz.1 + offset.z))
            .with_scale_xyz(size.x, size.y, size.z));
        world.insert(w, MeshHandle("cube".into()));
        world.insert(w, MaterialHandle("rpg_dungeon".into()));
        world.insert(w, RigidBody::static_body());
        world.insert(w, Collider::aabb(Vec3::splat(0.5)));
    }

    for (idx, &y) in floors_y.iter().enumerate() {
        let b = world.spawn();
        world.insert(b, Name(format!("ElevatorButton_{}", idx)));
        world.insert(b, Transform::at(Vec3::new(xz.0 + 2.5, y + 1.0, xz.1)).with_scale(0.25));
        world.insert(b, MeshHandle("cube".into()));
        world.insert(b, MaterialHandle("emissive".into()));
        world.insert(b, Spinner::new(Vec3::Y, 1.5));
        world.insert(b, Trigger::repeatable(1.2, TriggerAction::CallElevator {
            elevator: plat,
            floor_idx: idx as u32,
        }));
    }

    log::info!("Elevator spawned at ({:.1}, {:.1}) with {} floors, speed {:.1} m/s",
        xz.0, xz.1, floors_y.len(), speed);
}

struct DemoGame {
    camera: Camera3D,
    systems: Vec<Box<dyn System>>,
    postfx: PostFx,
    spawned: bool,
    dragging: bool,
    show_grid: bool,
    show_culling: bool,
    orbit_phase: f32,

    gltf_instances: Vec<GltfInstance>,
    skeletons: HashMap<String, Skeleton>,
    animations: HashMap<String, AnimationClip>,

    animation_runtime: AnimationRuntime,
    animation_events: AnimationEvents,

    bell_entity: Option<Entity>,

    lod_stats: [usize; 4],
    rpg: RpgState,
    prev_world_matrices: HashMap<Entity, glam::Mat4>,
}

impl DemoGame {
    fn new() -> Self {
        Self {
            camera: Camera3D::new(16.0 / 9.0),
            systems: vec![
                Box::new(TimerSystem),
                Box::new(RotationSystem),
                Box::new(MovementSystem),
                Box::new(ElevatorSystem),
                Box::new(SlidingDoorSystem),
            ],
            postfx: PostFx {
                bloom_threshold: 1.2,
                bloom_strength: 0.6,
                bloom_knee: 0.5,
                bloom_radius: 1.0,
                exposure: 0.6,
                ssao_strength: 0.8,
                ssao_radius: 0.6,
                ibl_strength: 0.15,
                debug_view: DebugView::Final,
                fxaa_strength: 1.0,
                fog_color: [0.55, 0.62, 0.72],
                fog_density: 0.0,
                fog_height_base: 0.0,
                fog_height_falloff: 0.05,
                vignette_strength: 0.0,
                film_grain: 0.0,
                chromatic_aberration: 0.0,
                shadow_bias: 0.0015,
                shadow_normal_bias: 3.0,
                shadow_fade_start: 150.0,
                shadow_fade_end: 200.0,
                lod_bias: 1.0,
                lod_distances: [30.0, 80.0, 200.0, 500.0],
                taa_strength: 1.0,
                taa_sharpening: 0.1,
                volumetric_density: 0.001,
                volumetric_scattering: 0.4,
                volumetric_phase_g: 0.6,
            },
            spawned: false,
            dragging: false,
            show_grid: true,
            show_culling: true,
            orbit_phase: 0.0,
            gltf_instances: Vec::new(),
            skeletons: HashMap::new(),
            animations: HashMap::new(),
            animation_runtime: AnimationRuntime::new(),
            animation_events: AnimationEvents::new(),
            bell_entity: None,
            lod_stats: [0; 4],
            rpg: RpgState::new(),
            prev_world_matrices: HashMap::new(),
        }
    }

    fn rpg_interact_distance(&self) -> f32 { 4.0 }
}

impl Game for DemoGame {
    fn init(&mut self, _world: &mut World, renderer: &mut Renderer) {
        renderer.add_mesh("cube", Mesh::cube(&renderer.device, 1.0));
        renderer.add_mesh("sphere", Mesh::sphere(&renderer.device, 0.5, 16, 24));
        renderer.add_mesh("ground", Mesh::plane(&renderer.device, 200.0, 1));
        renderer.add_mesh("quad", Mesh::plane(&renderer.device, 2.0, 1));
        renderer.add_mesh("quad_xy", Mesh::plane_xy(&renderer.device, 2.0, 1));
        renderer.add_mesh("cylinder", Mesh::cylinder(&renderer.device, 0.5, 1.0, 24));
        renderer.add_mesh("cone",     Mesh::cone(&renderer.device, 0.5, 1.0, 24));
        renderer.add_mesh("capsule",  Mesh::capsule(&renderer.device, 0.4, 0.8, 6, 20));

        let mut data = vec![0u8; 64 * 64 * 4];
        for y in 0..64 {
            for x in 0..64 {
                let idx = (y * 64 + x) * 4;
                let c = if ((x / 8) + (y / 8)) % 2 == 0 { 220 } else { 60 };
                data[idx] = c; data[idx + 1] = c; data[idx + 2] = c; data[idx + 3] = 255;
            }
        }
        renderer.load_texture_rgba("checker", &data, 64, 64).expect("checker");

        renderer.add_material("ground",
            Material::new([0.55, 0.60, 0.55, 1.0]).with_metallic_roughness(0.0, 0.85));
        renderer.add_material("checker_red",
            Material::new([1.0, 0.35, 0.35, 1.0]).with_texture("checker").with_metallic_roughness(0.0, 0.5));
        renderer.add_material("checker_blue",
            Material::new([0.35, 0.55, 1.0, 1.0]).with_texture("checker").with_metallic_roughness(0.0, 0.5));
        renderer.add_material("flat_red",
            Material::new([1.0, 0.35, 0.35, 1.0]).with_metallic_roughness(0.0, 0.5));
        renderer.add_material("flat_blue",
            Material::new([0.35, 0.55, 1.0, 1.0]).with_metallic_roughness(0.0, 0.5));
        renderer.add_material("gold",
            Material::new([1.0, 0.85, 0.3, 1.0]).with_metallic_roughness(1.0, 0.25));
        renderer.add_material("emissive",
            Material::new([1.0, 1.0, 1.0, 1.0]).with_metallic_roughness(0.0, 0.5).with_emissive([2.5, 2.2, 0.6]));
        renderer.add_material("glass",
            Material::new([0.7, 0.85, 1.0, 0.35]).with_metallic_roughness(0.2, 0.05).with_alpha_mode(AlphaMode::Blend));
        renderer.add_material("foliage",
            Material::new([0.25, 0.75, 0.3, 1.0]).with_metallic_roughness(0.0, 0.7).with_double_sided(true));

        rpg::register_materials(renderer);

        match render::load_gltf_into(renderer, "assets/animated.glb", "anim") {
            Ok(loaded) => {
                println!("Loaded glTF: {} instances, {} skeletons, {} animations",
                    loaded.instances.len(), loaded.skeletons.len(), loaded.animations.len());
                self.gltf_instances = loaded.instances;
                self.skeletons = loaded.skeletons;
                self.animations = loaded.animations;
            }
            Err(e) => eprintln!("glTF not loaded ({}). Using only procedural meshes.", e),
        }

        let sidecar = "assets/animated.anim_events.ron";
        match AnimationEvents::from_file(sidecar) {
            Ok(ev) if !ev.is_empty() => {
                log::info!("Animation events: loaded {} clip(s) from {}",
                    ev.clip_count(), sidecar);
                self.animation_events = ev;
            }
            _ => {
                if let Some(clip_name) = self.animations.keys().next().cloned() {
                    self.animation_events.add(&clip_name,
                        AnimationEvent::new(0.25, "footstep").with_payload("footstep_grass"));
                    self.animation_events.add(&clip_name,
                        AnimationEvent::new(0.75, "footstep").with_payload("footstep_grass"));
                    log::info!("Animation events: 2 demo events on clip '{}'", clip_name);
                }
            }
        }
    }

    fn configure_input(&mut self, map: &mut InputMap) {
        map
            .bind("toggle_grid", Key::KeyG)
            .bind("toggle_culling", Key::KeyC)
            .bind("debug_final", Key::F1)
            .bind("debug_ssao", Key::F2)
            .bind("debug_gbuffer_normal", Key::F3)
            .bind("debug_gbuffer_depth", Key::F4)
            .bind("debug_hdr", Key::F5)
            .bind("debug_csm", Key::F6)
            .bind("reload_shaders", Key::F12)
            .bind_mouse("primary_fire", MouseBtn::Left);

        for act in [
            "toggle_grid", "toggle_culling",
            "debug_final", "debug_ssao",
            "debug_gbuffer_normal", "debug_gbuffer_depth",
            "debug_hdr", "debug_csm",
        ] {
            map.action_in_context(act, "editor");
        }
        map.action_in_context("primary_fire", "gameplay");
    }

    fn update(
        &mut self,
        world: &mut World,
        input: &Input,
        renderer: &mut Renderer,
        dt: f32,
    ) -> bool {
        if input.pressed("reload_shaders") {
            match renderer.reload_shaders() {
                Ok(()) => log::info!("Shaders reloaded"),
                Err(e) => log::error!("Shader reload failed: {}", e),
            }
        }

        let ctrl = input.key_down(KeyCode::ControlLeft) || input.key_down(KeyCode::ControlRight);
        let alt = input.key_down(KeyCode::AltLeft) || input.key_down(KeyCode::AltRight);
        let shift = input.key_down(KeyCode::ShiftLeft) || input.key_down(KeyCode::ShiftRight);
        let plain = !ctrl && !alt && !shift;

        if plain {
            if input.pressed("toggle_grid") { self.show_grid = !self.show_grid; }
            if input.pressed("toggle_culling") { self.show_culling = !self.show_culling; }
        }

        if input.pressed("debug_final") { self.postfx.debug_view = DebugView::Final; }
        if input.pressed("debug_ssao") { self.postfx.debug_view = DebugView::Ssao; }
        if input.pressed("debug_gbuffer_normal") { self.postfx.debug_view = DebugView::GbufferNormal; }
        if input.pressed("debug_gbuffer_depth") { self.postfx.debug_view = DebugView::GbufferDepth; }
        if input.pressed("debug_hdr") { self.postfx.debug_view = DebugView::HdrPreBloom; }
        if input.pressed("debug_csm") { self.postfx.debug_view = DebugView::CsmCascade0; }

        if !input.play_mode && !input.editor_flying {
            let lmb = input.mouse_down(MouseButton::Left) && !input.editor_captured;
            if lmb {
                let (dx, dy) = input.mouse_delta;
                if self.dragging { self.camera.orbit(dx * 0.005, dy * 0.005); }
                self.dragging = true;
            } else { self.dragging = false; }
            if input.scroll_delta.abs() > 0.01 {
                self.camera.zoom(input.scroll_delta * 0.05);
            }
            let speed = 8.0 * dt;
            let mut pan = (0.0, 0.0);
            if input.key_down(KeyCode::KeyW) { pan.1 -= speed; }
            if input.key_down(KeyCode::KeyS) { pan.1 += speed; }
            if input.key_down(KeyCode::KeyA) { pan.0 -= speed; }
            if input.key_down(KeyCode::KeyD) { pan.0 += speed; }
            if pan != (0.0, 0.0) { self.camera.pan(pan.0, pan.1); }
        }

        self.orbit_phase += dt * 0.5;
        let (sp, cp) = self.orbit_phase.sin_cos();
        let pts: Vec<Entity> = world.query::<PointLight>().map(|(e, _)| e).collect();
        if pts.len() >= 3 {
            if let Some(t) = world.get_mut::<Transform>(pts[1]) {
                t.position.x = cp * 12.0; t.position.z = sp * 12.0;
            }
            if let Some(t) = world.get_mut::<Transform>(pts[2]) {
                t.position.x = -cp * 12.0; t.position.z = -sp * 12.0;
            }
        }

        if !self.spawned {
            self.spawned = true;
            rpg::spawn_scene(world);

            {
                let e = world.spawn();
                world.insert(e, Name("Sun".into()));
                world.insert(e, Transform::at(Vec3::new(0.0, 10.0, 0.0)));
                let mut sun = DirectionalLight::sun();
                sun.intensity = 3.0;
                world.insert(e, sun);
            }
            {
                let e = world.spawn();
                world.insert(e, Name("FillLight".into()));
                world.insert(e, Transform::at(Vec3::new(0.0, 10.0, 0.0)));
                world.insert(e, DirectionalLight::fill());
            }
            for (i, (pos, color, intensity, range)) in [
                ([0.0_f32, 3.0, 0.0], [1.0_f32, 0.4, 0.2], 4.0_f32, 18.0_f32),
                ([10.0, 4.0, 10.0], [0.2, 0.6, 1.0], 3.0, 14.0),
                ([-10.0, 4.0, -10.0], [0.4, 1.0, 0.4], 3.0, 14.0),
            ].iter().enumerate() {
                let e = world.spawn();
                world.insert(e, Name(format!("PointLight_{}", i)));
                world.insert(e, Transform::at(Vec3::from_array(*pos)));
                world.insert(e, PointLight::new(*color, *intensity, *range));
            }

            spawn_elevator(world, (8.0, 8.0), vec![0.0, 3.0, 6.0, 9.0], 2.5);

            for i in 0..5 {
                let e = world.spawn();
                let x = (i as f32 - 2.0) * 3.0;
                world.insert(e, Name(format!("BloodDecal_{}", i)));
                world.insert(e, Transform::at(Vec3::new(x, 0.06, 4.0))
                    .with_scale_xyz(2.0, 0.2, 2.0));
                world.insert(e, Decal {
                    texture: "checker".into(),
                    tint: [0.8, 0.05, 0.05, 0.9],
                });
            }

            {
                let bell = world.spawn();
                world.insert(bell, Name("Bell".into()));
                world.insert(bell, Transform::at(Vec3::new(5.0, 2.0, 5.0)).with_scale(0.3));
                world.insert(bell, MeshHandle("sphere".into()));
                world.insert(bell, MaterialHandle("emissive".into()));
                world.insert(bell, Spinner::new(Vec3::Y, 0.8));
                world.insert(bell, Timer::new(2.0));
                // ИЗМЕНЕНО (Фаза 4.2): колокольчик маршрутизируется на
                // шину Music для демонстрации работы шин. Открой
                // `all_bus_volumes` в UI/коде и покрути Music — ding
                // станет тише/громче независимо от выстрелов (SFX).
                world.insert(bell, AudioSource::new("ding")
                    .with_bus(AudioBus::Music)
                    .with_range(0.5, 15.0)
                    .with_volume(1.0));
                self.bell_entity = Some(bell);
                log::info!("Bell spawned at (5, 2, 5) on Music bus");
            }

            let mut index = 0;
            for inst in &self.gltf_instances {
                let e = world.spawn();
                let node_label = inst.node_name.clone().unwrap_or_else(|| format!("Gltf_{}", index));
                world.insert(e, Name(node_label));
                let (scale, rot, trans) = inst.model.to_scale_rotation_translation();
                let offset = Vec3::new((index as f32) * 3.0 - 3.0, 0.5, 30.0);
                world.insert(e, Transform::at(trans + offset).with_rotation(rot).with_scale(scale.x));
                world.insert(e, MeshHandle(inst.mesh_name.clone()));
                world.insert(e, MaterialHandle(inst.material_name.clone()));
                world.insert(e, TextureTiling::default());
                if let Some(skel_name) = &inst.skeleton_name {
                    world.insert(e, SkeletonHandle(skel_name.clone()));
                }
                if let Some(clip) = &inst.default_animation {
                    world.insert(e, AnimationPlayer::new(clip.clone()));
                }
                index += 1;
            }

            log::info!("Spawned RPG scene + lights + elevator + decals + bell");
        }

        if self.spawned {
            let player_pos = self.camera.position();

            let finished: Vec<Entity> = world.read_events::<TimerFinished>()
                .map(|ev| ev.entity).collect();
            for e in finished {
                if Some(e) == self.bell_entity {
                    if let Some(src) = world.get_mut::<AudioSource>(e) {
                        src.playing = true;
                    }
                    if let Some(t) = world.get_mut::<Timer>(e) { t.restart(); }
                }
            }

            let elevator_entities: Vec<Entity> =
                world.query::<Elevator>().map(|(e, _)| e).collect();
            for e in elevator_entities {
                let (sensor_r, center, half) = match (
                    world.get::<Elevator>(e).map(|el| el.sensor_radius),
                    world.get::<Transform>(e),
                ) {
                    (Some(r), Some(t)) => (r, t.position, t.scale.abs() * 0.5),
                    _ => continue,
                };

                let dx = player_pos.x - center.x;
                let dz = player_pos.z - center.z;
                let dist_xz = (dx * dx + dz * dz).sqrt();
                let in_xz = dist_xz < sensor_r + half.x.max(half.z);

                let dy = player_pos.y - center.y;
                let in_y = dy > -1.0 && dy < 3.0;

                if let Some(el) = world.get_mut::<Elevator>(e) {
                    el.player_inside = in_xz && in_y;
                }
            }

            rpg::tick(&mut self.rpg, world, player_pos, dt);

            if input.play_mode && input.key_pressed(KeyCode::KeyE) {
                let origin = self.camera.position();
                let dir = self.camera.forward();
                if let Some((target, dist)) = crate::editor::picking::pick_ray(world, renderer, origin, dir) {
                    if dist < self.rpg_interact_distance() {
                        let handled = rpg::try_interact(world, &mut self.rpg, target);
                        if !handled {
                            if let Some(&inter) = world.get::<Interactable>(target) {
                                match inter {
                                    Interactable::Pickup => { world.despawn(target); }
                                    Interactable::Paint(c) => { world.insert(target, Tint(c)); }
                                    Interactable::Toggle => {
                                        if let Some(sp) = world.get_mut::<Spinner>(target) {
                                            sp.speed = -sp.speed;
                                        } else if let Some(el) = world.get_mut::<Elevator>(target) {
                                            el.call(el.target_floor);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        for sys in self.systems.iter_mut() {
            sys.update(world, dt);
        }

        let anim_events = self.animation_runtime.advance_all(
            world, renderer,
            &self.animations, &self.skeletons, &self.animation_events,
            dt,
        );
        for ev in anim_events {
            log::debug!("Anim event: {} on #{} (clip '{}', payload {:?})",
                ev.name, ev.entity, ev.clip, ev.payload);
            world.send(ev);
        }

        true
    }

    fn apply_postfx(&mut self, postfx: PostFx) { self.postfx = postfx; }

    fn on_play_enter(&mut self, _world: &World) -> Option<Box<dyn Any>> {
        log::info!("Play-in-Editor: snapshotting RPG state");
        Some(Box::new(self.rpg.clone()))
    }

    fn on_play_exit(&mut self, state: Box<dyn Any>) {
        match state.downcast::<RpgState>() {
            Ok(rpg) => { self.rpg = *rpg; log::info!("Play-in-Editor: RPG state restored"); }
            Err(_) => { log::warn!("Play-in-Editor: failed to downcast RPG state"); }
        }
    }

    fn collect_draws(&mut self, world: &mut World, renderer: &Renderer) -> Vec<MeshDraw> {
        use std::collections::HashMap;
        type BucketKey = (String, String, [u32; 4], bool, bool, [u32; 2]);
        let mut buckets: HashMap<BucketKey, Vec<InstanceData>> = HashMap::new();
        let planes = self.camera.frustum_planes();
        let cam_pos = self.camera.position();

        let lod_bias = self.postfx.lod_bias.max(0.01);
        let lod_dists = self.postfx.lod_distances;
        let mut lod_counts = [0usize; 4];

        let mut new_prev: HashMap<Entity, glam::Mat4> = HashMap::new();

        let entities: Vec<_> = world.entities().to_vec();
        for e in entities {
            if let Some(v) = world.get::<Visible>(e) { if !v.0 { continue; } }
            if world.has::<Decal>(e) { continue; }

            let (Some(_t), Some(m), Some(mat)) = (
                world.get::<Transform>(e),
                world.get::<MeshHandle>(e),
                world.get::<MaterialHandle>(e),
            ) else { continue };

            let model = crate::game::world_matrix(world, e);
            let prev_model = self.prev_world_matrices.get(&e).copied().unwrap_or(model);
            new_prev.insert(e, model);

            let Some(mesh) = renderer.meshes.get(&m.0) else { continue };

            if self.show_culling {
                let (center, radius) = mesh.world_bounds(&model);
                if !sphere_in_frustum(center, radius, &planes) { continue; }
            }

            let (world_center, world_radius) = mesh.world_bounds(&model);
            let dist = (world_center - cam_pos).length();
            let dist_effective = (dist - world_radius).max(0.0) / lod_bias;

            let lod_level = if mesh.lods.is_empty() { 0 }
                else if dist_effective < lod_dists[0] { 0 }
                else if dist_effective < lod_dists[1] || mesh.lods.len() < 1 { 1 }
                else if dist_effective < lod_dists[2] || mesh.lods.len() < 2 { 2.min(mesh.lods.len()) }
                else { 3.min(mesh.lods.len()) };

            lod_counts[lod_level] += 1;

            let mesh_name = if lod_level == 0 { m.0.clone() }
                else { format!("{}__lod{}", m.0, lod_level - 1) };

            let material = renderer.materials.get(&mat.0).unwrap_or_else(|| renderer.materials_default());
            let blend = material.alpha_mode == AlphaMode::Blend;
            let double_sided = material.double_sided;

            let color = world.get::<Tint>(e).map(|t| t.0).unwrap_or([1.0, 1.0, 1.0, 1.0]);
            let tiling_size = world.get::<TextureTiling>(e).map(|t| t.size).unwrap_or(1.0);
            let uv_scale = compute_uv_scale(mesh, &model, tiling_size);
            let uv_key = [
                (uv_scale[0] * 1000.0).round() as i32 as u32,
                (uv_scale[1] * 1000.0).round() as i32 as u32,
            ];

            let key = (mesh_name, mat.0.clone(), color.map(f32::to_bits), blend, double_sided, uv_key);
            let inst = InstanceData::new_full(model, prev_model, color, uv_scale);
            buckets.entry(key).or_default().push(inst);
        }

        self.prev_world_matrices = new_prev;
        self.lod_stats = lod_counts;

        buckets.into_iter().map(|((mesh, material_name, _, blend, double_sided, _), instances)| MeshDraw {
            mesh,
            instances,
            texture: Some(material_name),
            blend,
            double_sided,
        }).collect()
    }

    fn collect_lines(&mut self, world: &mut World, renderer: &Renderer, selected: &[Entity]) -> Vec<LineVertex> {
        let mut batch = LineBatch::new();
        if self.show_grid {
            batch.grid(100.0, 2.0, [0.15, 0.18, 0.22, 1.0], [0.35, 0.40, 0.48, 1.0], 5);
            batch.axes(5.0);
        }
        for &e in selected {
            if let Some(v) = world.get::<Visible>(e) { if !v.0 { continue; } }
            if let (Some(_t), Some(mh)) = (world.get::<Transform>(e), world.get::<MeshHandle>(e)) {
                if let Some(mesh) = renderer.meshes.get(&mh.0) {
                    let model = crate::game::world_matrix(world, e);
                    let (center, radius) = mesh.world_bounds(&model);
                    batch.sphere_wireframe(center, radius * 1.05, [1.0, 0.85, 0.2, 1.0], 24);
                }
            }
        }
        batch.vertices().to_vec()
    }

    fn dir_lights(&self, world: &World) -> Vec<GpuLight> {
        world.query::<DirectionalLight>().map(|(_, l)| GpuLight {
            direction: [l.direction.x, l.direction.y, l.direction.z, l.intensity],
            color: [l.color[0], l.color[1], l.color[2], 0.0],
        }).take(4).collect()
    }

    fn point_lights(&self, world: &World) -> Vec<GpuPointLight> {
        world.query::<PointLight>().filter_map(|(e, l)| {
            let t = world.get::<Transform>(e)?;
            Some(GpuPointLight {
                position: [t.position.x, t.position.y, t.position.z, l.range],
                color: [l.color[0], l.color[1], l.color[2], l.intensity],
            })
        }).take(16).collect()
    }

    fn ambient(&self) -> [f32; 3] { [0.03, 0.035, 0.05] }
    fn postfx(&self) -> PostFx { self.postfx }
    fn camera(&self) -> &Camera3D { &self.camera }
    fn camera_mut(&mut self) -> &mut Camera3D { &mut self.camera }

    fn rpg_hud(&self) -> Vec<(String, String)> { self.rpg.hud_lines() }
    fn on_kill(&mut self, world: &mut World, target: Entity) { rpg::on_kill(world, &mut self.rpg, target); }

    fn save_game_state(&self) -> Option<String> {
        match ron::ser::to_string(&self.rpg) {
            Ok(s) => Some(s),
            Err(e) => { log::warn!("Failed to serialize RpgState: {}", e); None }
        }
    }

    fn load_game_state(&mut self, ron_str: &str) {
        match ron::from_str::<RpgState>(ron_str) {
            Ok(s) => {
                self.rpg = s;
                log::info!("RpgState restored: gold={}, keys={}, quests={}",
                    self.rpg.gold, self.rpg.keys.len(), self.rpg.quests.len());
            }
            Err(e) => log::error!("Failed to parse RpgState: {}", e),
        }
    }
}

fn main() {
    run(DemoGame::new());
}