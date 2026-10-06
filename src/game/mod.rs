pub mod animation;
pub mod audio;
pub mod components;
pub mod decals;
pub mod lights;
pub mod rpg;
pub mod timers;

pub use animation::{
    AnimationEvent, AnimationEventTriggered, AnimationEvents, AnimationEventsFile,
    AnimationRuntime,
};
pub use audio::{attenuation, source_position, AudioBus, AudioSource};
pub use decals::Decal;
pub use components::{
    AnimationPlayer, Chase, Elevator, ElevatorState, Health, Interactable, MaterialHandle,
    MeshHandle, Name, Parent, SkeletonHandle, SlidingDoor, Spinner, TextureTiling, Tint,
    Transform, Trigger, TriggerAction, Velocity, Visible,
};
pub use lights::{DirectionalLight, PointLight};
pub use timers::{Timer, TimerFinished, TimerMode, TimerSystem};

use glam::Mat4;
use crate::ecs::{Entity, World};

pub fn world_matrix(world: &World, entity: Entity) -> Mat4 {
    let mut mat = world
        .get::<Transform>(entity)
        .map(|t| t.matrix())
        .unwrap_or(Mat4::IDENTITY);

    let mut current = entity;
    let mut depth = 0;
    while let Some(Parent(p)) = world.get::<Parent>(current).copied() {
        if depth > 32 { break; }
        let parent_mat = world
            .get::<Transform>(p)
            .map(|t| t.matrix())
            .unwrap_or(Mat4::IDENTITY);
        mat = parent_mat * mat;
        current = p;
        depth += 1;
    }
    mat
}

/// См. полный doc-комментарий в оригинальном файле (#17).
pub fn world_position(world: &World, entity: Entity) -> Option<glam::Vec3> {
    world.get::<Transform>(entity)?;
    Some(world_matrix(world, entity).transform_point3(glam::Vec3::ZERO))
}