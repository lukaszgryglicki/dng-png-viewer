use crate::{
    console::Console,
    images::{Inspection, LoadedImage},
    loader::ImageLoader,
    signals::StopSignals,
    view::{self, Effect, Key, Mode, Size, View},
};
use anyhow::{Context, Result, anyhow, ensure};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::{Keycode, Mod},
    pixels::PixelFormatEnum,
};
use std::{
    ffi::OsStr,
    path::PathBuf,
    sync::Arc,
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

pub fn selected_driver(
    explicit: Option<&OsStr>,
    display: Option<&OsStr>,
    wayland: Option<&OsStr>,
) -> Result<String> {
    let driver = match explicit.filter(|value| !value.is_empty()) {
        Some(value) => value
            .to_str()
            .context("SDL_VIDEODRIVER must be valid UTF-8")?,
        None => video_driver(display, wayland),
    };
    ensure!(
        !driver.contains(','),
        "select a single SDL_VIDEODRIVER; console access cannot be a fallback"
    );
    Ok(if driver.eq_ignore_ascii_case("kmsdrm") {
        "KMSDRM".into()
    } else {
        driver.to_owned()
    })
}

pub fn key(key: Keycode) -> Option<Key> {
    match key {
        Keycode::Left => Some(Key::Left),
        Keycode::Right => Some(Key::Right),
        Keycode::Up => Some(Key::Up),
        Keycode::Down => Some(Key::Down),
        Keycode::Z => Some(Key::Native),
        Keycode::X => Some(Key::Fit),
        Keycode::Escape | Keycode::Q => Some(Key::Quit),
        _ => None,
    }
}

pub fn key_press(code: Keycode, modifiers: Mod) -> Option<Key> {
    if code == Keycode::C && modifiers.intersects(Mod::LCTRLMOD | Mod::RCTRLMOD) {
        Some(Key::Quit)
    } else {
        key(code)
    }
}

fn shutdown_signals() -> Result<StopSignals> {
    let signals =
        StopSignals::install(&[signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM])?;
    ensure!(
        sdl2::hint::set_with_priority("SDL_NO_SIGNAL_HANDLERS", "1", &sdl2::hint::Hint::Override),
        "cannot configure orderly shutdown signals"
    );
    Ok(signals)
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
                window_label(view.shift),
                window_label(view.shift + 8.0),
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

fn window_label(shift: f64) -> String {
    format!("{shift:.6}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

pub fn run(paths: Vec<PathBuf>, preload: usize, brightness_step: f64) -> Result<()> {
    ensure!(!paths.is_empty(), "cannot view an empty playlist");
    ensure!(
        brightness_step.is_finite() && brightness_step > 0.0 && brightness_step <= 8.0,
        "brightness step must be finite, greater than 0 and at most 8 bits"
    );
    let shutdown = shutdown_signals()?;
    let driver = selected_driver(
        std::env::var_os("SDL_VIDEODRIVER").as_deref(),
        std::env::var_os("DISPLAY").as_deref(),
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
    )?;
    ensure!(
        sdl2::hint::set_with_priority("SDL_VIDEODRIVER", &driver, &sdl2::hint::Hint::Override),
        "cannot select SDL video driver {driver}"
    );
    let mut console = Console::open(&driver)?;
    // All SDL objects must be destroyed before the guard can release a pending VT switch.
    let result = run_sdl(paths, preload, brightness_step, console.as_mut(), &shutdown);
    match console {
        Some(console) => console.finish(result),
        None => result,
    }
}

fn run_sdl(
    paths: Vec<PathBuf>,
    preload: usize,
    brightness_step: f64,
    mut console: Option<&mut Console>,
    shutdown: &StopSignals,
) -> Result<()> {
    if shutdown.stopping() || console.as_ref().is_some_and(|console| console.stopping()) {
        return Ok(());
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

    let loader = ImageLoader::new(paths.clone(), preload)?;
    let mut index = 0usize;
    let mut generation = 0u64;
    loader.request(generation, index)?;
    let mut image: Option<Arc<LoadedImage>> = None;
    let mut view = View::default();
    let mut pending_native = false;
    let mut pending_window = None;
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
        if shutdown.stopping() || console.as_ref().is_some_and(|console| console.stopping()) {
            break;
        }
        let mut batch = Vec::new();
        if let Some(event) = events.wait_event_timeout(if present { 0 } else { 30 }) {
            batch.push(event);
        }
        batch.extend(events.poll_iter());
        let mut pressed_keys = if let Some(console) = console.as_mut() {
            console.keys()?
        } else {
            Vec::new()
        };
        for event in batch {
            let pressed = match event {
                Event::Quit { .. } => Some(Key::Quit),
                Event::KeyDown {
                    keycode: Some(code),
                    keymod,
                    ..
                } if console.is_none() => key_press(code, keymod),
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
                        view.clamp_pan(image.as_deref().map(LoadedImage::size), size);
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
            if let Some(pressed) = pressed {
                pressed_keys.push(pressed);
            }
        }
        for pressed in pressed_keys {
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
                        if let Some(loaded) = image.take() {
                            pending_window = match &loaded.info {
                                Inspection::Dr(info) => Some((info.max_shift, view.shift)),
                                Inspection::Standard { .. } => None,
                            };
                        }
                        view = View::default();
                        pending_native = false;
                        message = "Loading...".into();
                        loader.request(generation, index)?;
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
        if shutdown.stopping() || console.as_ref().is_some_and(|console| console.stopping()) {
            break;
        }
        while let Some((completed, result)) = loader.try_recv()? {
            if completed != generation {
                continue;
            }
            match result {
                Ok(loaded) => {
                    view = View::new(&loaded.info);
                    view.brightness_step = brightness_step;
                    if let (Inspection::Dr(info), Some((max_shift, shift))) =
                        (&loaded.info, pending_window.take())
                        && info.max_shift == max_shift
                        && (0.0..=f64::from(max_shift)).contains(&shift)
                    {
                        view.shift = shift;
                    }
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
                    pending_window = None;
                }
            }
            render = true;
            present = true;
            hud_visible = true;
            hud_until = Instant::now() + Duration::from_secs(4);
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
            let caption = title(index, &paths, image.as_deref(), &view, &message);
            canvas
                .window_mut()
                .set_title(&caption)
                .context("updating window title")?;
            shown.clone_from(&base);
            if hud_visible {
                let controls = if view.mode == Mode::Native || pending_native {
                    "1:1: arrows pan | X fit | ESC/Q/Ctrl+C exit"
                } else {
                    "Arrows: previous/next, brighter/darker | Z 1:1 | ESC/Q/Ctrl+C exit"
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
    drop(loader);
    ensure!(
        !failed,
        "one or more images could not be decoded; see diagnostics above"
    );
    Ok(())
}

#[cfg(all(test, target_os = "freebsd"))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Run by make test-display on its own Xvfb and PTY; never on a physical console"]
    fn console_input_pipeline() {
        let image = std::env::var_os("DNG_VIEWER_TEST_IMAGE").expect("isolated image fixture");
        assert_eq!(std::env::var("SDL_VIDEODRIVER").unwrap(), "x11");
        let shutdown = shutdown_signals().unwrap();
        let (mut console, before) = Console::test_pty().unwrap();
        let result = run_sdl(
            vec![image.into()],
            3,
            view::DEFAULT_BRIGHTNESS_STEP,
            Some(&mut console),
            &shutdown,
        );
        console.finish(result).unwrap();
        let mut after = unsafe { std::mem::zeroed::<libc::termios>() };
        assert_eq!(
            unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut after) },
            0
        );
        assert_eq!(after.c_cc, before.c_cc);
        assert_eq!(
            (
                after.c_iflag,
                after.c_oflag,
                after.c_cflag,
                after.c_lflag,
                after.c_ispeed,
                after.c_ospeed
            ),
            (
                before.c_iflag,
                before.c_oflag,
                before.c_cflag,
                before.c_lflag,
                before.c_ispeed,
                before.c_ospeed
            )
        );
    }
}
