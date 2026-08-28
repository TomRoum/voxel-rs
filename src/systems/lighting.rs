use std::collections::VecDeque;

use rustc_hash::FxHashMap;

use crate::gamelogic::content::blocks;
use crate::world::chunk::{BlockId, ChunkPos, VoxelLight};
use crate::world::world::World;

pub const MAX_LIGHT_WORK: usize = 4096;

#[derive(Clone, Copy)]
struct LightNode {
    x: i32,
    y: i32,
    z: i32,
    light: VoxelLight,
}

pub struct Lighting {
    additions: VecDeque<LightNode>,
    sources: FxHashMap<(i32, i32, i32), VoxelLight>,
    work_budget: usize,
}

impl Default for Lighting {
    fn default() -> Self {
        Self::new(MAX_LIGHT_WORK)
    }
}

impl Lighting {
    pub fn new(work_budget: usize) -> Self {
        Self { additions: VecDeque::new(), sources: FxHashMap::default(), work_budget }
    }

    pub fn seed_block_light(&mut self, world: &mut World, x: i32, y: i32, z: i32, light: VoxelLight) {
        let source = self.sources.entry((x, y, z)).or_default();
        *source = source.max(light);
        let current = world.get_light(x, y, z);
        let merged = current.max(light);
        if merged != current && world.set_light(x, y, z, merged) {
            self.additions.push_back(LightNode { x, y, z, light: merged });
        }
    }

    pub fn block_changed(&mut self, world: &mut World, x: i32, y: i32, z: i32, old: BlockId, new: BlockId) {
        let source_removed = old == blocks::GLOWSTONE && new != blocks::GLOWSTONE;
        let opacity_changed = is_transparent(old) != is_transparent(new)
            || (old == blocks::AIR) != (new == blocks::AIR);
        if source_removed {
            self.sources.remove(&(x, y, z));
        }
        if source_removed || opacity_changed {
            self.rebuild(world);
        }
        if new == blocks::GLOWSTONE {
            self.seed_block_light(world, x, y, z, VoxelLight::new(0, 15, 15, 15, 15));
        }
    }

    fn rebuild(&mut self, world: &mut World) {
        let sources = self.sources.clone();
        self.additions.clear();
        world.clear_lights();
        for ((x, y, z), light) in sources {
            self.seed_block_light(world, x, y, z, light);
        }
    }

    pub fn seed_sky_column(&mut self, world: &mut World, x: i32, top_y: i32, z: i32) {
        let block = world.get_block(x, top_y, z);
        if block == blocks::AIR || is_transparent(block) {
            self.seed_block_light(world, x, top_y, z, VoxelLight::new(VoxelLight::MAX, 0, 0, 0, 0));
        }
    }

    pub fn seed_sky_chunk(&mut self, world: &mut World, pos: ChunkPos) {
        let above = ChunkPos::new(pos.x, pos.y + 1, pos.z);
        if world.has_chunk(&above) {
            return;
        }
        let top_y = (pos.y + 1) * 32 - 1;
        for z in 0..32 {
            for x in 0..32 {
                self.seed_sky_column(world, pos.x * 32 + x, top_y, pos.z * 32 + z);
            }
        }
    }

    pub fn chunk_loaded(&mut self, world: &World, pos: ChunkPos) {
        for y in 0..32 {
            for z in 0..32 {
                for x in [0, 31] {
                    self.enqueue_existing_light(world, pos.x * 32 + x, pos.y * 32 + y, pos.z * 32 + z);
                }
            }
        }
        for y in 0..32 {
            for x in 0..32 {
                for z in [0, 31] {
                    self.enqueue_existing_light(world, pos.x * 32 + x, pos.y * 32 + y, pos.z * 32 + z);
                }
            }
        }
        for z in 0..32 {
            for x in 0..32 {
                for y in [0, 31] {
                    self.enqueue_existing_light(world, pos.x * 32 + x, pos.y * 32 + y, pos.z * 32 + z);
                }
            }
        }
        for y in 0..32 {
            for z in 0..32 {
                for x in [pos.x * 32 - 1, (pos.x + 1) * 32] {
                    self.enqueue_existing_light(world, x, pos.y * 32 + y, pos.z * 32 + z);
                }
            }
        }
        for y in 0..32 {
            for x in 0..32 {
                for z in [pos.z * 32 - 1, (pos.z + 1) * 32] {
                    self.enqueue_existing_light(world, pos.x * 32 + x, pos.y * 32 + y, z);
                }
            }
        }
        for z in 0..32 {
            for x in 0..32 {
                for y in [pos.y * 32 - 1, (pos.y + 1) * 32] {
                    self.enqueue_existing_light(world, pos.x * 32 + x, y, pos.z * 32 + z);
                }
            }
        }
    }

    fn enqueue_existing_light(&mut self, world: &World, x: i32, y: i32, z: i32) {
        let light = world.get_light(x, y, z);
        if light != VoxelLight::default() {
            self.additions.push_back(LightNode { x, y, z, light });
        }
    }

    pub fn process(&mut self, world: &mut World) -> usize {
        let mut processed = 0;
        while processed < self.work_budget {
            let Some(node) = self.additions.pop_front() else { break };
            processed += 1;

            for (dx, dy, dz) in [(1, 0, 0), (-1, 0, 0), (0, 1, 0), (0, -1, 0), (0, 0, 1), (0, 0, -1)] {
                let pos = (node.x + dx, node.y + dy, node.z + dz);
                let block = world.get_block(pos.0, pos.1, pos.2);
                if block == blocks::AIR || is_transparent(block) {
                    let next = attenuate(node.light);
                    if next != VoxelLight::default() {
                        let current = world.get_light(pos.0, pos.1, pos.2);
                        let merged = current.max(next);
                        if merged != current && world.set_light(pos.0, pos.1, pos.2, merged) {
                            self.additions.push_back(LightNode { x: pos.0, y: pos.1, z: pos.2, light: merged });
                        }
                    }
                }
            }
        }
        processed
    }

    pub fn has_pending_work(&self) -> bool {
        !self.additions.is_empty()
    }
}

fn attenuate(light: VoxelLight) -> VoxelLight {
    VoxelLight::new(
        light.sky().saturating_sub(1),
        light.block().saturating_sub(1),
        light.red().saturating_sub(1),
        light.green().saturating_sub(1),
        light.blue().saturating_sub(1),
    )
}

fn is_transparent(block: BlockId) -> bool {
    matches!(block, blocks::GLASS | blocks::WATER | blocks::OAK_LEAVES)
}

#[cfg(test)]
mod tests {
    use super::Lighting;
    use crate::gamelogic::content::blocks;
    use crate::world::chunk::{Chunk, ChunkPos, ChunkStorageAllocator, VoxelLight};
    use crate::world::world::World;

    #[test]
    fn addition_propagates_with_channel_falloff() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(128);
        lighting.seed_block_light(&mut world, 16, 16, 16, VoxelLight::new(0, 15, 15, 8, 0));
        lighting.process(&mut world);

        assert_eq!(world.get_light(16, 16, 16).block(), 15);
        assert_eq!(world.get_light(17, 16, 16).red(), 14);
        assert_eq!(world.get_light(18, 16, 16).green(), 6);
        assert_eq!(world.get_light(19, 16, 16).red(), 12);
        assert_eq!(world.get_light(19, 16, 16).green(), 5);
    }

    #[test]
    fn seed_merges_channels_without_regression() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(0);

        lighting.seed_block_light(&mut world, 16, 16, 16, VoxelLight::new(0, 0, 15, 2, 0));
        lighting.seed_block_light(&mut world, 16, 16, 16, VoxelLight::new(0, 0, 3, 12, 9));

        assert_eq!(world.get_light(16, 16, 16), VoxelLight::new(0, 0, 15, 12, 9));
    }

    #[test]
    fn dimmer_seed_does_not_decrease_existing_light() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(0);

        lighting.seed_block_light(&mut world, 16, 16, 16, VoxelLight::new(0, 15, 14, 13, 12));
        lighting.seed_block_light(&mut world, 16, 16, 16, VoxelLight::new(0, 4, 3, 2, 1));

        assert_eq!(world.get_light(16, 16, 16), VoxelLight::new(0, 15, 14, 13, 12));
    }

    #[test]
    fn sky_column_propagates_downward_with_falloff() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(128);

        lighting.seed_sky_column(&mut world, 16, 31, 16);
        lighting.process(&mut world);

        assert_eq!(world.get_light(16, 31, 16).sky(), 15);
        assert_eq!(world.get_light(16, 30, 16).sky(), 14);
        assert_eq!(world.get_light(16, 29, 16).sky(), 13);
    }

    #[test]
    fn opaque_block_stops_sky_seed() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        for x in 0..32 {
            for z in 0..32 {
                world.set_block(x, 20, z, blocks::STONE);
            }
        }
        let mut lighting = Lighting::new(128);

        lighting.seed_sky_column(&mut world, 16, 31, 16);
        lighting.process(&mut world);

        assert_eq!(world.get_light(16, 20, 16).sky(), 0);
        assert_eq!(world.get_light(16, 19, 16).sky(), 0);
    }

    #[test]
    fn removing_isolated_emitter_clears_propagated_light() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(256);
        let source = VoxelLight::new(0, 15, 15, 15, 15);

        lighting.seed_block_light(&mut world, 16, 16, 16, source);
        lighting.process(&mut world);
        assert_ne!(world.get_light(18, 16, 16), VoxelLight::default());

        lighting.block_changed(&mut world, 16, 16, 16, blocks::GLOWSTONE, blocks::AIR);
        while lighting.has_pending_work() { lighting.process(&mut world); }
        assert_eq!(world.get_light(18, 16, 16), VoxelLight::default());
    }

    #[test]
    fn removing_one_overlapping_emitter_preserves_the_other() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(1024);

        lighting.seed_block_light(&mut world, 12, 16, 16, VoxelLight::new(0, 15, 15, 0, 0));
        lighting.seed_block_light(&mut world, 20, 16, 16, VoxelLight::new(0, 0, 0, 15, 15));
        while lighting.has_pending_work() { lighting.process(&mut world); }
        let before = world.get_light(16, 16, 16);

        lighting.block_changed(&mut world, 12, 16, 16, blocks::GLOWSTONE, blocks::AIR);
        while lighting.has_pending_work() { lighting.process(&mut world); }

        assert!(world.get_light(16, 16, 16).green() >= before.green());
        assert!(world.get_light(16, 16, 16).blue() >= before.blue());
        assert_eq!(world.get_light(16, 16, 16).red(), 0);
    }

    #[test]
    fn loaded_neighbor_receives_boundary_light() {
        let allocator = ChunkStorageAllocator::new();
        let mut world = World::new();
        world.set_chunk(Chunk::new(ChunkPos::new(0, 0, 0), 5, allocator.allocate()));
        let mut lighting = Lighting::new(256);

        lighting.seed_block_light(&mut world, 30, 16, 16, VoxelLight::new(0, 15, 0, 0, 0));
        while lighting.has_pending_work() { lighting.process(&mut world); }
        assert_eq!(world.get_light(32, 16, 16), VoxelLight::default());

        world.set_chunk(Chunk::new(ChunkPos::new(1, 0, 0), 5, allocator.allocate()));
        lighting.chunk_loaded(&world, ChunkPos::new(1, 0, 0));
        while lighting.has_pending_work() { lighting.process(&mut world); }
        assert_eq!(world.get_light(32, 16, 16).block(), 13);
    }
}