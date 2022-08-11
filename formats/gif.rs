use std::borrow::{Borrow, Cow};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::BufWriter;
use std::rc::Rc;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use color_quant::NeuQuant;
use futures_lite::future;
use gif::{Encoder, Frame, Repeat};
use rayon::prelude::*;
use wgpu::TextureFormat;

use crate::data::{ActiveRecorders, CaptureRecording, HasTaskStatus, TextureFrame};
use crate::image_utils::{frame_data_to_rgba_image, to_rgba};

pub struct RecordGif;
pub type CaptureGifRecording = CaptureRecording<RecordGif>;

#[derive(Component)]
pub struct SaveGifRecording(Task<()>);
impl HasTaskStatus for SaveGifRecording {
	fn is_done(&mut self) -> bool {
		let result = future::block_on(future::poll_once(&mut self.0));
		result.is_some()
	}
}

pub fn quantize_frames(
	width: u16,
	height: u16,
	frames: VecDeque<TextureFrame>,
	format: TextureFormat,
) -> Vec<Frame<'static>> {
	log::info!("Starting quantize");
	frames
		.into_par_iter()
		.map(|frame| {
			let formatted = to_rgba(frame.texture, format);
			let quant = NeuQuant::new(20, 256, formatted.as_slice());
			let mut index_cache = fnv::FnvHashMap::default();
			let pixels: Vec<u8> = formatted
				.chunks(4)
				.map(|pixel| {
					*(index_cache
						.entry(pixel)
						.or_insert_with(|| quant.index_of(pixel) as u8))
				})
				.collect();

			let mut output = Frame::default();
			// GIF delay is increments of 10ms in u16; duration gives millis in u128.
			// Convert to GIF delay scale then do a capped conversion to u16
			output.delay = (frame.frame_time.as_millis() / 10).min(u16::MAX as u128) as u16;
			output.palette = Some(quant.color_map_rgb());
			output.transparent = None;

			output.left = 0;
			output.top = 0;
			output.width = width;
			output.height = height;

			output.buffer = Cow::Owned(pixels);

			output
		})
		.collect()
}

pub fn capture_gif_recording(
	mut commands: Commands,
	mut recorders: ResMut<ActiveRecorders>,
	mut events: ResMut<Events<CaptureRecording<RecordGif>>>,
	images: Res<Assets<Image>>,
) {
	let thread_pool = AsyncComputeTaskPool::get();
	'event_drain: for event in events.drain() {
		if let Some(mut recorder) = recorders.get_mut(&event.tracking_id) {
			let (target_size, target_format) = match images.get(&recorder.target_handle) {
				Some(image) => (image.size(), image.texture_descriptor.format),
				None => continue 'event_drain,
			};

			let frames = std::mem::replace(&mut recorder.frames, VecDeque::new());
			let task = thread_pool.spawn(async move {
				let target_size = target_size;
				let target_format = target_format;
				let frames = frames;

				let out_buffer = std::fs::File::create("test.gif").unwrap();
				let mut writer = BufWriter::new(out_buffer);
				log::info!("Create encoder");
				match gif::Encoder::new(writer, target_size.x as u16, target_size.y as u16, &[]) {
					Ok(mut encoder) => {
						log::info!("Got encoder");
						encoder.set_repeat(Repeat::Infinite);
						let frames = quantize_frames(
							target_size.x as u16,
							target_size.y as u16,
							frames,
							target_format,
						);
						log::info!("Done quantize");

						// for mut data in frames {
						// 	let formatted = to_rgba(data.texture, target_format);
						// 	let quant = NeuQuant::new(15, 256, formatted.texture.as_slice());
						//
						// 	let frame_data = frame_data_to_rgba_image(data.texture, target_format);
						// 	let frame = Frame::default();
						// 	frame.delay = encoder.write_frame(frame).unwrap();
						// }
						log::info!("Start write frames");

						for frame in frames {
							encoder.write_frame(&frame).unwrap();
						}

						log::info!("Wrote thing");
					}
					Err(e) => {
						log::error!("{}", e);
					}
				};

				log::info!("DONE");

				()
			});

			commands.spawn().insert(SaveGifRecording(task));
		}
	}
}
