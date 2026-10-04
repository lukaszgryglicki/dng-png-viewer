use crate::{
    images::{self, Inspection, LoadedImage},
    view::{self, Effect, Key, Mode, Size, View},
};
use anyhow::{Context, Result, anyhow, ensure};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::Keycode,
    pixels::PixelFormatEnum,
};
use std::{
    ffi::OsStr,
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub fn video_driver(display: Option<&OsStr>, wayland: Option<&OsStr>) -> &'static str {
    if display.is_some_and(|value| !value.is_empty()) {
        "x11"
    } else if wayland.is_some_and(|value| !value.is_empty()) {
        "wayland"
    } else {
        "KMSDRM"
    }
}

pub fn key(key: Keycode) -> Option<Key> {
    match key {
        Keycode::Left => Some(Key::Left),
        Keycode::Right => Some(Key::Right),
        Keycode::Up => Some(Key::Up),
        Keycode::Down => Some(Key::Down),
        Keycode::Z => Some(Key::Native),
        Keycode::X => Some(Key::Fit),
        Keycode::Escape => Some(Key::Quit),
        _ => None,
    }
}

fn label(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

pub fn title(
    index: usize,
    paths: &[PathBuf],
    image: Option<&LoadedImage>,
    view: &View,
    message: &str,
) -> String {
    let detail = if let Some(image) = image {
        let mode = if view.mode == Mode::Native {
            "1:1"
        } else {
            "FIT"
        };
        match &image.info {
            Inspection::Dr(info) => format!(
                "{mode} | Gray16 {}x{} | {} code bits | window {}-{}",
                info.width,
                info.height,
                info.code_bits,
                view.shift,
                view.shift + 8,
            ),
            Inspection::Standard { width, height, .. } => {
                format!("{mode} | standard {width}x{height}")
            }
        }
    } else {
        message.to_owned()
    };
    format!(
        "dng-png-viewer | {}/{} | {} | {detail}",
        index + 1,
        paths.len(),
        label(&paths[index])
    )
}

pub fn run(paths: Vec<PathBuf>) -> Result<()> {
    ensure!(!paths.is_empty(), "cannot view an empty playlist");
    if std::env::var_os("SDL_VIDEODRIVER").is_none() {
        let driver = video_driver(
            std::env::var_os("DISPLAY").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        );
        ensure!(
            sdl2::hint::set("SDL_VIDEODRIVER", driver),
            "cannot select SDL video driver {driver}"
        );
    }
    let sdl = sdl2::init().map_err(|error| anyhow!("initializing SDL2: {error}"))?;
    let video = sdl.video().map_err(|error| anyhow!("opening native video output: {error}; X11 needs DISPLAY, console mode needs a KMSDRM-capable SDL2 and device access"))?;
    let mode = video
        .desktop_display_mode(0)
        .map_err(|error| anyhow!("reading display mode: {error}"))?;
    let window = video
        .window("dng-png-viewer", mode.w.max(1) as u32, mode.h.max(1) as u32)
        .position_centered()
        .resizable()
        .allow_highdpi()
        .fullscreen_desktop()
        .build()
        .context("creating fullscreen window")?;
    let mut canvas = window
        .into_canvas()
        .build()
        .context("creating native renderer")?;
    sdl.mouse().show_cursor(false);
    let creator = canvas.texture_creator();
    let (width, height) = canvas
        .output_size()
        .map_err(|error| anyhow!("reading drawable size: {error}"))?;
    let mut size = Size { width, height };
    size.frame_bytes()?;
    let mut texture = creator
        .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
        .context("creating viewport texture")?;
    let mut events = sdl
        .event_pump()
        .map_err(|error| anyhow!("opening native input: {error}"))?;

    let (request_tx, request_rx) = mpsc::channel::<(u64, PathBuf)>();
    let (result_tx, result_rx) = mpsc::channel();
    let _loader = thread::Builder::new()
        .name("image-loader".into())
        .spawn(move || {
            while let Ok(mut request) = request_rx.recv() {
                while let Ok(newer) = request_rx.try_recv() {
                    request = newer;
                }
                let result = images::decode(&request.1);
                if result_tx.send((request.0, result)).is_err() {
                    break;
                }
            }
        })
        .context("starting image loader")?;
    let mut index = 0usize;
    let mut generation = 0u64;
    request_tx.send((generation, paths[index].clone()))?;
    let mut image: Option<LoadedImage> = None;
    let mut view = View::default();
    let mut pending_native = false;
    let mut message = "Loading...".to_owned();
    let mut base = vec![0; size.frame_bytes()?];
    let mut shown = base.clone();
    let mut render = true;
    let mut present = true;
    let mut hud_until = Instant::now() + Duration::from_secs(4);
    let mut hud_visible = true;
    let mut failed = false;
    let mut quitting = false;
    eprintln!(
        "Native SDL2 {} output; {} image(s)",
        video.current_video_driver(),
        paths.len()
    );
    while !quitting {
        while let Ok((completed, result)) = result_rx.try_recv() {
            if completed != generation {
                continue;
            }
            match result {
                Ok(loaded) => {
                    view = View::new(&loaded.info);
                    if pending_native {
                        view.mode = Mode::Native;
                    }
                    image = Some(loaded);
                    message.clear();
                }
                Err(error) => {
                    failed = true;
                    message = format!("Cannot load image: {error:#}");
                    eprintln!("{:?}: {message}", paths[index]);
                    image = None;
                }
            }
            render = true;
            present = true;
            hud_visible = true;
            hud_until = Instant::now() + Duration::from_secs(4);
        }
        let mut batch = Vec::new();
        if let Some(event) = events.wait_event_timeout(if present { 0 } else { 30 }) {
            batch.push(event);
        }
        batch.extend(events.poll_iter());
        for event in batch {
            let pressed = match event {
                Event::Quit { .. } => Some(Key::Quit),
                Event::KeyDown {
                    keycode: Some(code),
                    ..
                } => key(code),
                Event::Window {
                    win_event: WindowEvent::Resized(..) | WindowEvent::SizeChanged(..),
                    ..
                } => {
                    let (width, height) = canvas
                        .output_size()
                        .map_err(|error| anyhow!("reading resized drawable: {error}"))?;
                    if width > 0 && height > 0 && (width != size.width || height != size.height) {
                        size = Size { width, height };
                        size.frame_bytes()?;
                        texture = creator
                            .create_texture_streaming(PixelFormatEnum::RGB24, width, height)
                            .context("resizing viewport texture")?;
                        view.clamp_pan(image.as_ref().map(LoadedImage::size), size);
                        render = true;
                        present = true;
                    }
                    None
                }
                Event::Window {
                    win_event: WindowEvent::Exposed,
                    ..
                } => {
                    present = true;
                    None
                }
                _ => None,
            };
            let Some(pressed) = pressed else {
                continue;
            };
            let effect = if let Some(loaded) = &image {
                view.handle(pressed, &loaded.info, loaded.size(), size)
            } else {
                match pressed {
                    Key::Quit => Effect::Quit,
                    Key::Native => {
                        pending_native = true;
                        Effect::None
                    }
                    Key::Fit => {
                        pending_native = false;
                        Effect::None
                    }
                    Key::Left if !pending_native => Effect::Previous,
                    Key::Right if !pending_native => Effect::Next,
                    _ => Effect::None,
                }
            };
            match effect {
                Effect::Quit => {
                    quitting = true;
                    break;
                }
                Effect::Previous | Effect::Next => {
                    let next = if effect == Effect::Previous {
                        index.saturating_sub(1)
                    } else {
                        (index + 1).min(paths.len() - 1)
                    };
                    if next != index {
                        index = next;
                        generation += 1;
                        image = None;
                        view = View::default();
                        pending_native = false;
                        message = "Loading...".into();
                        request_tx
                            .send((generation, paths[index].clone()))
                            .context("requesting next image")?;
                        render = true;
                    }
                }
                Effect::Redraw => render = true,
                Effect::None => {}
            }
            hud_visible = true;
            hud_until = Instant::now() + Duration::from_secs(4);
            present = true;
        }
        if quitting {
            break;
        }
        if hud_visible && Instant::now() >= hud_until && image.is_some() {
            hud_visible = false;
            present = true;
        }
        if render {
            base = if let Some(loaded) = &image {
                view::render(loaded, &view, size)?
            } else {
                vec![0; size.frame_bytes()?]
            };
            render = false;
            present = true;
        }
        if present {
            let caption = title(index, &paths, image.as_ref(), &view, &message);
            canvas
                .window_mut()
                .set_title(&caption)
                .context("updating window title")?;
            shown.clone_from(&base);
            if hud_visible {
                let controls = if view.mode == Mode::Native || pending_native {
                    "1:1: arrows pan | X fit | ESC exit"
                } else {
                    "Arrows: previous/next, brighter/darker | Z 1:1 | ESC exit"
                };
                view::draw_hud(&mut shown, size, &[&caption, controls]);
            }
            texture
                .update(None, &shown, size.width as usize * 3)
                .context("uploading viewport pixels")?;
            canvas.clear();
            canvas
                .copy(&texture, None, None)
                .map_err(|error| anyhow!("presenting viewport: {error}"))?;
            canvas.present();
            present = false;
        }
    }
    drop(request_tx);
    ensure!(
        !failed,
        "one or more images could not be decoded; see diagnostics above"
    );
    Ok(())
}
