extern crate vulkano;

mod bbox;
mod pair_counter;
mod pair_writer;
use self::bbox::{BBox, BBoxFinder};
use self::pair_counter::PairCounter;
use self::pair_writer::PairWriter;

use std::sync::Arc;

pub struct Grid {
    pub bbox: BBox,
    pub resolution: [u32; 3],
    pub cell_size: [f32; 3],
    pub cells_buffer: Arc<vulkano::buffer::BufferAccess + Send + Sync>,
    pub references_buffer: Arc<vulkano::buffer::BufferAccess + Send + Sync>,
}

pub struct GridBuilder {
    queue: Arc<vulkano::device::Queue>,
    bbox_finder: BBoxFinder,
    pair_counter: PairCounter,
    pair_writer: PairWriter,
    triangle_count: usize,
}

impl GridBuilder {
    pub fn new(
        queue: Arc<vulkano::device::Queue>,
        positions: Arc<vulkano::buffer::BufferAccess + Send + Sync>,
        indices: Arc<vulkano::buffer::BufferAccess + Send + Sync>,
        triangle_count: usize,
    ) -> GridBuilder {
        let bbox_finder = BBoxFinder::new(queue.clone(), positions.clone(), triangle_count);
        let pair_counter = PairCounter::new(
            queue.clone(),
            positions.clone(),
            indices.clone(),
            triangle_count,
        );
        let pair_writer = PairWriter::new(queue.clone(), triangle_count);
        GridBuilder {
            queue,
            bbox_finder,
            pair_counter,
            pair_writer,
            triangle_count,
        }
    }

    pub fn build(
        &mut self,
        future: Box<vulkano::sync::GpuFuture>,
    ) -> (Grid, Box<vulkano::sync::GpuFuture>) {
        let bbox = self.bbox_finder.calculate_bbox(self.queue.clone(), future);

        let dx = bbox.max.position[0] - bbox.min.position[0];
        let dy = bbox.max.position[1] - bbox.min.position[1];
        let dz = bbox.max.position[2] - bbox.min.position[2];

        let grid_size = [dx, dy, dz];
        let resolution = calc_grid_reolution(&grid_size, self.triangle_count);
        let cell_size = [
            dx / resolution[0] as f32,
            dy / resolution[1] as f32,
            dz / resolution[2] as f32,
        ];

        let count_pairs_result = self.pair_counter.count_pairs(
            self.queue.clone(),
            bbox.min.position,
            cell_size,
            resolution,
        );
        let (cells_buffer, references_buffer, future) =
            self.pair_writer
                .write_pairs(self.queue.clone(), count_pairs_result, resolution);

        (
            Grid {
                bbox,
                resolution,
                cell_size,
                cells_buffer,
                references_buffer,
            },
            future,
        )
    }
}

fn calc_grid_reolution(grid_size: &[f32; 3], triangle_count: usize) -> [u32; 3] {
    let volume = grid_size[0] * grid_size[1] * grid_size[2];
    let k = (5.0 * triangle_count as f32 / volume).powf(1.0 / 3.0);
    let nx = (grid_size[0] * k).floor().max(1.0) as u32;
    let ny = (grid_size[1] * k).floor().max(1.0) as u32;
    let nz = (grid_size[2] * k).floor().max(1.0) as u32;
    [nx, ny, nz]
}
