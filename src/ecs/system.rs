use super::world::World;

pub trait System: Send + Sync {
    fn update(&mut self, world: &mut World, dt: f32);
}