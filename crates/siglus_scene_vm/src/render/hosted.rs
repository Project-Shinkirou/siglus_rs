use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Debug)]
pub(super) struct OffscreenTarget {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    readback: wgpu::Buffer,
    width: u32,
    height: u32,
    padded_row: u32,
    completion: Arc<AtomicU64>,
    map_generation: u64,
    captured_generation: Option<u64>,
}

impl OffscreenTarget {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("siglus-hosted-output"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_row = (width * 4).div_ceil(alignment) * alignment;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("siglus-hosted-readback"),
            size: u64::from(padded_row) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            texture,
            view,
            readback,
            width,
            height,
            padded_row,
            completion: Arc::new(AtomicU64::new(0)),
            map_generation: 0,
            captured_generation: None,
        }
    }

    fn submit(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        generation: u64,
    ) -> Result<()> {
        let map_generation = self
            .map_generation
            .checked_add(1)
            .filter(|value| *value <= u64::MAX >> 2)
            .context("hosted readback epoch exhausted")?;
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("siglus-hosted-readback"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.padded_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit(Some(encoder.finish()));
        self.map_generation = map_generation;
        self.captured_generation = Some(generation);
        let pending = (map_generation << 2) | 1;
        self.completion.store(pending, Ordering::Release);
        let completion = Arc::clone(&self.completion);
        self.readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let done = (map_generation << 2) | if result.is_ok() { 2 } else { 3 };
                let _ =
                    completion.compare_exchange(pending, done, Ordering::AcqRel, Ordering::Acquire);
            });
        Ok(())
    }

    fn cancel(&mut self) {
        let status = self.completion.swap(0, Ordering::AcqRel) & 3;
        if self.captured_generation.take().is_some() && status != 3 {
            self.readback.unmap();
        }
    }

    fn poll(&mut self, device: &wgpu::Device, pixels: &mut Vec<u8>) -> Result<Option<u64>> {
        let generation = self
            .captured_generation
            .context("hosted readback not pending")?;
        device.poll(wgpu::PollType::Poll);
        match self.completion.load(Ordering::Acquire) & 3 {
            1 => return Ok(None),
            2 => {}
            _ => {
                self.cancel();
                anyhow::bail!("map hosted readback");
            }
        }
        let data = self.readback.slice(..).get_mapped_range()?;
        let row_bytes = self.width as usize * 4;
        pixels.clear();
        pixels.reserve(row_bytes * self.height as usize);
        for row in data
            .chunks_exact(self.padded_row as usize)
            .take(self.height as usize)
        {
            pixels.extend_from_slice(&row[..row_bytes]);
        }
        drop(data);
        self.cancel();
        Ok(Some(generation))
    }
}

impl Drop for OffscreenTarget {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Renderer {
    pub async fn new_offscreen(width: u32, height: u32) -> Result<Self> {
        let instance = wgpu::Instance::default();
        Self::new_from_instance_surface(instance, None, width, height, 1.0).await
    }

    /// Advances only after a native draw reaches the hosted output, not on VM ticks or reads.
    pub fn hosted_frame_generation(&self) -> u64 {
        self.hosted_frame_generation
    }

    /// Submits once, then polls without waiting. The revision belongs to the copied image.
    pub fn poll_frame_rgba(&mut self, pixels: &mut Vec<u8>) -> Result<Option<(u32, u32, u64)>> {
        let output = self
            .offscreen
            .as_mut()
            .context("renderer has no hosted output")?;
        if output.captured_generation.is_none() {
            output.submit(&self.device, &self.queue, self.hosted_frame_generation)?;
            return Ok(None);
        }
        Ok(output
            .poll(&self.device, pixels)?
            .map(|generation| (output.width, output.height, generation)))
    }

    pub fn cancel_frame_readback(&mut self) {
        if let Some(output) = self.offscreen.as_mut() {
            output.cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a hardware GPU adapter"]
    fn aborted_map_is_an_error_and_canceled_callbacks_cannot_complete_the_next_map() {
        let mut renderer = pollster::block_on(Renderer::new_offscreen(63, 47)).unwrap();
        let mut pixels = vec![17, 19, 23];
        assert!(renderer.poll_frame_rgba(&mut pixels).unwrap().is_none());
        let output = renderer.offscreen.as_mut().unwrap();
        // Abort the real pending map; wgpu delivers BufferAsyncError.
        output.readback.unmap();
        assert!(renderer.poll_frame_rgba(&mut pixels).is_err());
        assert_eq!(pixels, [17, 19, 23]);
        assert!(renderer.poll_frame_rgba(&mut pixels).unwrap().is_none());
        renderer.cancel_frame_readback();
        assert!(renderer.poll_frame_rgba(&mut pixels).unwrap().is_none());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while renderer.poll_frame_rgba(&mut pixels).unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(pixels, vec![0; 63 * 47 * 4]);
    }
}
