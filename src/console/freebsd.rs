use super::{Key, Keyboard, Result};
use crate::signals::StopSignals;
use anyhow::{Context, anyhow, ensure};
use signal_hook::consts::signal;
use std::{
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    time::Instant,
};

const KD_TEXT: i32 = 0;
const KD_GRAPHICS: i32 = 1;
const K_XLATE: i32 = 1;
const VT_AUTO: u8 = 0;
const VT_PROCESS: u8 = 1;
const CONSOLE_SIGNALS: &[i32] = &[
    signal::SIGUSR1,
    signal::SIGUSR2,
    signal::SIGINT,
    signal::SIGTERM,
    signal::SIGHUP,
    signal::SIGQUIT,
    signal::SIGTSTP,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
struct VtMode {
    mode: u8,
    waitv: u8,
    release: i16,
    acquire: i16,
    force: i16,
}

const fn request<T>(direction: libc::c_ulong, group: u8, number: u8) -> libc::c_ulong {
    direction
        | ((size_of::<T>() as libc::c_ulong) << 16)
        | ((group as libc::c_ulong) << 8)
        | number as libc::c_ulong
}

// FreeBSD sys/consio.h and sys/kbio.h; Linux uses different ioctl encodings.
const KDGETMODE: libc::c_ulong = request::<i32>(0x40000000, b'K', 9);
const KDSETMODE: libc::c_ulong = request::<i32>(0x20000000, b'K', 10);
const KDGKBMODE: libc::c_ulong = request::<i32>(0x40000000, b'K', 6);
const KDSKBMODE: libc::c_ulong = request::<i32>(0x20000000, b'K', 7);
const VT_GETMODE: libc::c_ulong = request::<VtMode>(0x40000000, b'v', 3);
const VT_SETMODE: libc::c_ulong = request::<VtMode>(0x80000000, b'v', 2);
const VT_GETACTIVE: libc::c_ulong = request::<i32>(0x40000000, b'v', 7);
const VT_GETINDEX: libc::c_ulong = request::<i32>(0x40000000, b'v', 8);

struct Snapshot {
    video: i32,
    keyboard: i32,
    switching: VtMode,
    attributes: libc::termios,
    index: i32,
    active: i32,
    foreground: bool,
}

impl Snapshot {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.index > 0 && self.index == self.active,
            "KMSDRM requires the active physical console; this VT is {}, active VT is {}",
            self.index,
            self.active
        );
        ensure!(
            self.foreground,
            "KMSDRM must run in the foreground of its physical console"
        );
        ensure!(
            self.video == KD_TEXT && self.keyboard == K_XLATE && self.switching.mode == VT_AUTO,
            "refusing to take over a graphics/owned console; use an ordinary text VT, not Xorg's VT"
        );
        Ok(())
    }
}

trait Device {
    fn active(&mut self) -> Result<i32>;
    fn switching(&mut self, value: VtMode) -> Result<()>;
    fn attributes(&mut self, value: &libc::termios) -> Result<()>;
    fn video(&mut self, value: i32) -> Result<()>;
    fn keyboard(&mut self, value: i32) -> Result<()>;
}

struct Terminal(File);

impl Terminal {
    fn open() -> Result<(Self, Snapshot)> {
        let terminal = Self(OpenOptions::new().read(true).write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open("/dev/tty")
            .context("KMSDRM requires an active physical console, not SSH, a pipe or a terminal emulator")?);
        let mut attributes = unsafe { std::mem::zeroed::<libc::termios>() };
        // All buffers have the exact native ABI expected by these read-only ioctls.
        let fd = terminal.0.as_raw_fd();
        syscall(unsafe { libc::tcgetattr(fd, &mut attributes) })
            .context("reading console terminal attributes")?;
        let foreground = unsafe { libc::tcgetpgrp(fd) };
        ensure!(
            foreground >= 0,
            "reading console foreground group: {}",
            io::Error::last_os_error()
        );
        let snapshot = Snapshot {
            index: terminal.read_ioctl(VT_GETINDEX).context(
                "KMSDRM requires a physical FreeBSD VT; no graphics devices were opened",
            )?,
            active: terminal
                .read_ioctl(VT_GETACTIVE)
                .context("reading active console")?,
            video: terminal
                .read_ioctl(KDGETMODE)
                .context("reading console video mode")?,
            keyboard: terminal
                .read_ioctl(KDGKBMODE)
                .context("reading console keyboard mode")?,
            switching: terminal
                .read_ioctl(VT_GETMODE)
                .context("reading console VT ownership")?,
            attributes,
            foreground: foreground == unsafe { libc::getpgrp() },
        };
        snapshot.validate()?;
        Ok((terminal, snapshot))
    }

    fn read_ioctl<T: Default>(&self, command: libc::c_ulong) -> Result<T> {
        let mut value = T::default();
        syscall(unsafe { libc::ioctl(self.0.as_raw_fd(), command, &mut value) })?;
        Ok(value)
    }

    fn value_ioctl(&self, command: libc::c_ulong, value: i32) -> Result<()> {
        syscall(unsafe { libc::ioctl(self.0.as_raw_fd(), command, value as libc::c_ulong) })
    }
}

fn syscall(result: libc::c_int) -> Result<()> {
    if result < 0 {
        Err(io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

impl Device for Terminal {
    fn active(&mut self) -> Result<i32> {
        self.read_ioctl(VT_GETACTIVE)
            .context("checking console after claiming VT ownership")
    }

    fn switching(&mut self, value: VtMode) -> Result<()> {
        syscall(unsafe { libc::ioctl(self.0.as_raw_fd(), VT_SETMODE, &value) })
            .context("setting console VT ownership")
    }

    fn attributes(&mut self, value: &libc::termios) -> Result<()> {
        syscall(unsafe { libc::tcsetattr(self.0.as_raw_fd(), libc::TCSAFLUSH, value) })
            .context("setting console terminal attributes")
    }

    fn video(&mut self, value: i32) -> Result<()> {
        self.value_ioctl(KDSETMODE, value)
            .context("setting console video mode")
    }

    fn keyboard(&mut self, value: i32) -> Result<()> {
        self.value_ioctl(KDSKBMODE, value)
            .context("restoring console keyboard mode")
    }
}

struct Guard<D: Device> {
    device: D,
    saved: Snapshot,
    owned: bool,
    raw: bool,
    graphics: bool,
    keyboard: bool,
}

impl<D: Device> Guard<D> {
    fn enter(device: D, saved: Snapshot) -> Result<Self> {
        saved.validate()?;
        let mut guard = Self {
            device,
            saved,
            owned: false,
            raw: false,
            graphics: false,
            keyboard: false,
        };
        guard.device.switching(VtMode {
            mode: VT_PROCESS,
            release: signal::SIGUSR1 as i16,
            acquire: signal::SIGUSR2 as i16,
            ..VtMode::default()
        })?;
        guard.owned = true;
        guard.keyboard = true;
        ensure!(
            guard.device.active()? == guard.saved.index,
            "active console changed during startup; no graphics devices were opened"
        );
        let mut raw = guard.saved.attributes;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        guard.device.attributes(&raw)?;
        guard.raw = true;
        guard.device.video(KD_GRAPHICS)?;
        guard.graphics = true;
        Ok(guard)
    }

    fn restore(&mut self) -> Result<()> {
        let mut errors = Vec::new();
        let mut restore = |changed: &mut bool, result: Result<()>| match result {
            Ok(()) => *changed = false,
            Err(error) => errors.push(format!("{error:#}")),
        };
        if self.keyboard {
            restore(
                &mut self.keyboard,
                self.device.keyboard(self.saved.keyboard),
            );
        }
        if self.raw {
            restore(
                &mut self.raw,
                self.device.attributes(&self.saved.attributes),
            );
        }
        if self.graphics {
            restore(&mut self.graphics, self.device.video(self.saved.video));
        }
        // VT_AUTO completes a pending switch: restore it only after SDL and the tty are restored.
        if self.owned {
            restore(&mut self.owned, self.device.switching(self.saved.switching));
        }
        ensure!(
            errors.is_empty(),
            "console cleanup failed: {}",
            errors.join("; ")
        );
        Ok(())
    }
}

impl<D: Device> Drop for Guard<D> {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            eprintln!("error: {error:#}");
        }
    }
}

pub struct Console {
    guard: Guard<Terminal>,
    signals: StopSignals,
    keyboard: Keyboard,
}

impl Console {
    #[cfg(test)]
    pub(crate) fn test_pty() -> Result<(Self, libc::termios)> {
        let mut device = Terminal(
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open("/dev/tty")?,
        );
        let error = device
            .read_ioctl::<i32>(VT_GETINDEX)
            .err()
            .context("test refuses to modify a physical console")?;
        ensure!(
            error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.raw_os_error() == Some(libc::ENOTTY)),
            "test requires a PTY, not a physical console"
        );
        let signals = StopSignals::install(CONSOLE_SIGNALS)?;
        let mut attributes = unsafe { std::mem::zeroed::<libc::termios>() };
        syscall(unsafe { libc::tcgetattr(device.0.as_raw_fd(), &mut attributes) })?;
        let mut raw = attributes;
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        raw.c_cc[libc::VMIN] = 0;
        raw.c_cc[libc::VTIME] = 0;
        device.attributes(&raw)?;
        Ok((
            Self {
                guard: Guard {
                    device,
                    saved: Snapshot {
                        video: 0,
                        keyboard: 0,
                        switching: VtMode::default(),
                        attributes,
                        index: 0,
                        active: 0,
                        foreground: true,
                    },
                    owned: false,
                    raw: true,
                    graphics: false,
                    keyboard: false,
                },
                signals,
                keyboard: Keyboard::default(),
            },
            attributes,
        ))
    }

    pub fn open(driver: &str) -> Result<Option<Self>> {
        if driver != "KMSDRM" {
            return Ok(None);
        }
        ensure!(
            std::env::var_os("SDL_INPUT_FREEBSD_KEEP_KBD").is_some(),
            "console keyboard preservation must be configured before starting threads"
        );
        let (device, saved) = Terminal::open()?;
        let signals = StopSignals::install(CONSOLE_SIGNALS)?;
        let guard = Guard::enter(device, saved)?;
        Ok(Some(Self {
            guard,
            signals,
            keyboard: Keyboard::default(),
        }))
    }

    pub fn stopping(&self) -> bool {
        self.signals.stopping()
    }

    pub fn keys(&mut self) -> Result<Vec<Key>> {
        if self.stopping() {
            return Ok(vec![Key::Quit]);
        }
        let now = Instant::now();
        let mut keys = self.keyboard.expire(now);
        let mut bytes = [0; 256];
        for _ in 0..16 {
            match self.guard.device.0.read(&mut bytes) {
                Ok(0) => break,
                Ok(count) => keys.extend(self.keyboard.feed(&bytes[..count], now)),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    if self.stopping() {
                        return Ok(vec![Key::Quit]);
                    }
                }
                Err(error) => return Err(error).context("reading console keys"),
            }
        }
        Ok(keys)
    }

    pub fn finish(mut self, result: Result<()>) -> Result<()> {
        match (result, self.guard.restore()) {
            (Ok(()), cleanup) | (cleanup, Ok(())) => cleanup,
            (Err(error), Err(cleanup)) => Err(anyhow!("{error:#}; {cleanup:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, rc::Rc};

    #[derive(Default)]
    struct History {
        events: Vec<&'static str>,
        fail: Option<&'static str>,
        switched: bool,
    }

    struct Fake(Rc<RefCell<History>>);

    impl Fake {
        fn event(&mut self, value: &'static str) -> Result<()> {
            let mut history = self.0.borrow_mut();
            history.events.push(value);
            if history.fail == Some(value) {
                history.fail = None;
                anyhow::bail!("injected {value} failure");
            }
            Ok(())
        }
    }

    impl Device for Fake {
        fn active(&mut self) -> Result<i32> {
            Ok(if self.0.borrow().switched { 9 } else { 2 })
        }

        fn switching(&mut self, value: VtMode) -> Result<()> {
            self.event(if value.mode == VT_AUTO {
                "auto"
            } else {
                "owned"
            })
        }
        fn attributes(&mut self, value: &libc::termios) -> Result<()> {
            self.event(if value.c_lflag & libc::ICANON == 0 {
                "raw"
            } else {
                "cooked"
            })
        }
        fn video(&mut self, value: i32) -> Result<()> {
            self.event(if value == KD_GRAPHICS {
                "graphics"
            } else {
                "text"
            })
        }
        fn keyboard(&mut self, value: i32) -> Result<()> {
            assert_eq!(value, K_XLATE);
            self.event("keyboard")
        }
    }

    fn saved() -> Snapshot {
        let mut attributes = unsafe { std::mem::zeroed::<libc::termios>() };
        attributes.c_lflag = libc::ICANON | libc::ECHO;
        Snapshot {
            video: KD_TEXT,
            keyboard: K_XLATE,
            switching: VtMode::default(),
            attributes,
            index: 2,
            active: 2,
            foreground: true,
        }
    }

    #[test]
    fn invalid_console_states_fail_before_any_mutation() {
        for case in 0..6 {
            let mut state = saved();
            match case {
                0 => state.active = 9,
                1 => state.index = 0,
                2 => state.foreground = false,
                3 => state.video = KD_GRAPHICS,
                4 => state.keyboard = 0,
                5 => state.switching.mode = VT_PROCESS,
                _ => unreachable!(),
            }
            let log = Rc::new(RefCell::new(History::default()));
            assert!(Guard::enter(Fake(Rc::clone(&log)), state).is_err());
            assert!(log.borrow().events.is_empty());
        }
    }

    #[test]
    fn a_switch_during_preflight_is_rejected_before_graphics_initialization() {
        let log = Rc::new(RefCell::new(History {
            switched: true,
            ..History::default()
        }));
        let result = Guard::enter(Fake(Rc::clone(&log)), saved());
        assert!(result.is_err());
        assert_eq!(log.borrow().events, ["owned", "keyboard", "auto"]);
    }

    #[test]
    fn own_vt_before_graphics_and_release_only_after_display_and_input_restore() {
        let log = Rc::new(RefCell::new(History::default()));
        let guard = Guard::enter(Fake(Rc::clone(&log)), saved()).unwrap();
        log.borrow_mut().events.push("SDL resources destroyed");
        drop(guard);
        assert_eq!(
            log.borrow().events,
            [
                "owned",
                "raw",
                "graphics",
                "SDL resources destroyed",
                "keyboard",
                "cooked",
                "text",
                "auto",
            ]
        );
    }

    #[test]
    fn partial_startup_failure_rolls_back_owned_modes() {
        for (failure, expected) in [
            ("owned", vec!["owned"]),
            ("raw", vec!["owned", "raw", "keyboard", "auto"]),
            (
                "graphics",
                vec!["owned", "raw", "graphics", "keyboard", "cooked", "auto"],
            ),
        ] {
            let log = Rc::new(RefCell::new(History {
                fail: Some(failure),
                ..History::default()
            }));
            assert!(Guard::enter(Fake(Rc::clone(&log)), saved()).is_err());
            assert_eq!(log.borrow().events, expected);
        }
    }

    #[test]
    fn cleanup_error_does_not_skip_other_restoration_or_leave_vt_owned() {
        for failure in ["keyboard", "cooked", "text", "auto"] {
            let log = Rc::new(RefCell::new(History::default()));
            let mut guard = Guard::enter(Fake(Rc::clone(&log)), saved()).unwrap();
            log.borrow_mut().fail = Some(failure);
            assert!(guard.restore().unwrap_err().to_string().contains(failure));
            assert_eq!(
                &log.borrow().events[3..],
                ["keyboard", "cooked", "text", "auto"]
            );
            assert_eq!(guard.owned, failure == "auto");
            drop(guard);
            assert_eq!(log.borrow().events.last(), Some(&failure));
        }
    }

    #[test]
    fn unwinding_restores_the_console_and_cleanup_is_idempotent() {
        let log = Rc::new(RefCell::new(History::default()));
        let unwind = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = Guard::enter(Fake(Rc::clone(&log)), saved()).unwrap();
            panic!("simulated native renderer failure");
        }));
        assert!(unwind.is_err());
        assert_eq!(log.borrow().events.last(), Some(&"auto"));
        let mut guard = Guard::enter(Fake(Rc::clone(&log)), saved()).unwrap();
        guard.restore().unwrap();
        let length = log.borrow().events.len();
        guard.restore().unwrap();
        drop(guard);
        assert_eq!(log.borrow().events.len(), length);
    }

    #[test]
    fn ioctl_layout_matches_freebsd_headers() {
        assert_eq!(size_of::<VtMode>(), 8);
        assert_eq!(align_of::<VtMode>(), 2);
        assert_eq!(
            (KDGETMODE, KDSETMODE, KDGKBMODE, KDSKBMODE),
            (0x40044b09, 0x20044b0a, 0x40044b06, 0x20044b07)
        );
        assert_eq!(
            (VT_GETMODE, VT_SETMODE, VT_GETACTIVE, VT_GETINDEX),
            (0x40087603, 0x80087602, 0x40047607, 0x40047608)
        );
    }

    #[test]
    fn signals_request_orderly_exit_in_an_isolated_process() {
        const CHILD: &str = "DNG_VIEWER_SIGNAL_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "console::freebsd::tests::signals_request_orderly_exit_in_an_isolated_process",
                ])
                .env(CHILD, "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        for &signal in CONSOLE_SIGNALS {
            let signals = StopSignals::install(CONSOLE_SIGNALS).unwrap();
            assert_eq!(unsafe { libc::raise(signal) }, 0);
            assert!(signals.stopping());
        }
    }
}
