use std::collections::VecDeque;

use crate::gamelogic::content::blocks;
use crate::world::chunk::{BlockId, VoxelLight};
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
    work_budget: usize,
}

impl Default for Lighting {
    fn default() -> Self {
        Self::new(MAX_LIGHT_WORK)
    }
}

impl Lighting {
    pub fn new(work_budget: usize) -> Self {
        Self { additions: VecDeque::new(), work_budget }
    }

    pub fn seed_block_light(&mut self, world: &mut World, x: i32, y: i32, z: i32, light: VoxelLight) {
        if world.get_light(x, y, z).max(light) != world.get_light(x, y, z) && world.set_light(x, y, z, light) {
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
}