use crate::images::{GrayWindow, Inspection, LoadedImage};
use anyhow::{Result, ensure};
use font8x8::UnicodeFonts;
use image::DynamicImage;
use rayon::prelude::*;

pub const DEFAULT_BRIGHTNESS_STEP: f64 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub fn frame_bytes(self) -> Result<usize> {
        let pixels = u64::from(self.width) * u64::from(self.height);
        ensure!(
            pixels > 0 && pixels <= 33_554_432,
            "viewport must contain 1..33554432 pixels"
        );
        Ok(usize::try_from(pixels * 3)?)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Fit,
    Native,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Native,
    Fit,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    Redraw,
    Previous,
    Next,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub mode: Mode,
    pub shift: f64,
    pub brightness_step: f64,
    pub pan_x: i64,
    pub pan_y: i64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            mode: Mode::Fit,
            shift: 0.0,
            brightness_step: DEFAULT_BRIGHTNESS_STEP,
            pan_x: 0,
            pan_y: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    pub left: f64,
    pub top: f64,
    pub scale: f64,
    pub image: Size,
}

impl View {
    pub fn new(info: &Inspection) -> Self {
        Self {
            shift: match info {
                Inspection::Dr(info) => f64::from(info.initial_shift),
                _ => 0.0,
            },
            ..Self::default()
        }
    }

    pub fn layout(&self, image: Size, viewport: Size) -> Layout {
        if self.mode == Mode::Native {
            let axis = |source: u32, screen: u32, pan: i64| {
                let difference = i64::from(screen) - i64::from(source);
                let center = difference.div_euclid(2);
                if difference < 0 {
                    center.saturating_add(pan).clamp(difference, 0)
                } else {
                    center
                }
            };
            Layout {
                left: axis(image.width, viewport.width, self.pan_x) as f64,
                top: axis(image.height, viewport.height, self.pan_y) as f64,
                scale: 1.0,
                image,
            }
        } else {
            let scale = (f64::from(viewport.width) / f64::from(image.width))
                .min(f64::from(viewport.height) / f64::from(image.height));
            Layout {
                left: (f64::from(viewport.width) - f64::from(image.width) * scale) / 2.0,
                top: (f64::from(viewport.height) - f64::from(image.height) * scale) / 2.0,
                scale,
                image,
            }
        }
    }

    pub fn clamp_pan(&mut self, image: Option<Size>, viewport: Size) {
        if let Some(image) = image {
            let layout = self.layout(image, viewport);
            if self.mode == Mode::Native {
                self.pan_x = layout.left as i64
                    - (i64::from(viewport.width) - i64::from(image.width)).div_euclid(2);
                self.pan_y = layout.top as i64
                    - (i64::from(viewport.height) - i64::from(image.height)).div_euclid(2);
            }
        }
    }

    pub fn handle(&mut self, key: Key, info: &Inspection, image: Size, viewport: Size) -> Effect {
        let before = self.clone();
        match key {
            Key::Quit => return Effect::Quit,
            Key::Native => {
                self.mode = Mode::Native;
                self.pan_x = 0;
                self.pan_y = 0;
            }
            Key::Fit => {
                self.mode = Mode::Fit;
                self.pan_x = 0;
                self.pan_y = 0;
            }
            Key::Left if self.mode == Mode::Fit => return Effect::Previous,
            Key::Right if self.mode == Mode::Fit => return Effect::Next,
            Key::Up | Key::Down if self.mode == Mode::Fit => {
                if let Inspection::Dr(info) = info {
                    self.shift = if key == Key::Up {
                        (self.shift - self.brightness_step).max(0.0)
                    } else {
                        (self.shift + self.brightness_step).min(f64::from(info.max_shift))
                    };
                    // Keep whole-bit windows exact after repeated decimal steps.
                    let integer = self.shift.round();
                    let roundoff = (8.0 * f64::EPSILON).min(self.brightness_step / 2.0);
                    if (self.shift - integer).abs() <= roundoff {
                        self.shift = integer;
                    }
                }
            }
            Key::Left => self.pan_x = self.pan_x.saturating_add(64),
            Key::Right => self.pan_x = self.pan_x.saturating_sub(64),
            Key::Up => self.pan_y = self.pan_y.saturating_add(64),
            Key::Down => self.pan_y = self.pan_y.saturating_sub(64),
        }
        self.clamp_pan(Some(image), viewport);
        if *self == before {
            Effect::None
        } else {
            Effect::Redraw
        }
    }
}

pub fn render(image: &LoadedImage, view: &View, size: Size) -> Result<Vec<u8>> {
    let window = GrayWindow::new(view.shift)?;
    let mut frame = vec![0; size.frame_bytes()?];
    let layout = view.layout(image.size(), size);
    match &image.image {
        DynamicImage::ImageLuma16(pixels) => draw(&mut frame, size, layout, |x, y| {
            let value = window.sample(pixels.get_pixel(x, y).0[0]);
            [value, value, value, 255]
        }),
        DynamicImage::ImageRgba8(pixels) => {
            draw(&mut frame, size, layout, |x, y| pixels.get_pixel(x, y).0)
        }
        _ => anyhow::bail!("image was not prepared for display"),
    }
    Ok(frame)
}

fn draw<F: Fn(u32, u32) -> [u8; 4] + Sync>(
    frame: &mut [u8],
    size: Size,
    layout: Layout,
    sample: F,
) {
    let right = layout.left + f64::from(layout.image.width) * layout.scale;
    let bottom = layout.top + f64::from(layout.image.height) * layout.scale;
    frame
        .par_chunks_mut(size.width as usize * 3)
        .enumerate()
        .for_each(|(y, row)| {
            let cy = y as f64 + 0.5;
            if cy < layout.top || cy >= bottom {
                return;
            }
            let sy = ((cy - layout.top) / layout.scale - 0.5)
                .clamp(0.0, f64::from(layout.image.height - 1));
            let y0 = sy.floor() as u32;
            let y1 = (y0 + 1).min(layout.image.height - 1);
            let fy = sy - f64::from(y0);
            for (x, out) in row.chunks_exact_mut(3).enumerate() {
                let cx = x as f64 + 0.5;
                if cx < layout.left || cx >= right {
                    continue;
                }
                let sx = ((cx - layout.left) / layout.scale - 0.5)
                    .clamp(0.0, f64::from(layout.image.width - 1));
                let x0 = sx.floor() as u32;
                let x1 = (x0 + 1).min(layout.image.width - 1);
                let fx = sx - f64::from(x0);
                let samples = [
                    sample(x0, y0),
                    sample(x1, y0),
                    sample(x0, y1),
                    sample(x1, y1),
                ];
                let weights = [
                    (1.0 - fx) * (1.0 - fy),
                    fx * (1.0 - fy),
                    (1.0 - fx) * fy,
                    fx * fy,
                ];
                for channel in 0..3 {
                    let value: f64 = samples
                        .iter()
                        .zip(weights)
                        .map(|(pixel, weight)| {
                            f64::from(pixel[channel]) * (f64::from(pixel[3]) / 255.0) * weight
                        })
                        .sum();
                    out[channel] = value.round().clamp(0.0, 255.0) as u8;
                }
            }
        });
}

pub fn draw_hud(frame: &mut [u8], size: Size, lines: &[&str]) {
    let scale = if size.width >= 960 { 2usize } else { 1 };
    let pad = 8usize;
    let width = size.width as usize;
    let height = size.height as usize;
    if width <= pad * 2 || height <= pad * 2 {
        return;
    }
    let columns = (width - pad * 2) / (8 * scale);
    let rows = lines.len().min((height - pad * 2) / (10 * scale));
    let box_height = (pad * 2 + rows * 10 * scale).min(height);
    for pixel in frame[..box_height * width * 3].chunks_exact_mut(3) {
        for channel in pixel {
            *channel /= 3;
        }
    }
    for (line, text) in lines.iter().take(rows).enumerate() {
        for (column, character) in text.chars().take(columns).enumerate() {
            let glyph = font8x8::BASIC_FONTS
                .get(character)
                .or_else(|| font8x8::LATIN_FONTS.get(character))
                .unwrap_or_else(|| font8x8::BASIC_FONTS.get('?').expect("built-in glyph"));
            for (gy, bits) in glyph.into_iter().enumerate() {
                for gx in 0..8 {
                    if bits & (1 << gx) == 0 {
                        continue;
                    }
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let x = pad + column * 8 * scale + gx * scale + dx;
                            let y = pad + line * 10 * scale + gy * scale + dy;
                            let offset = (y * width + x) * 3;
                            frame[offset..offset + 3].fill(255);
                        }
                    }
                }
            }
        }
    }
}
