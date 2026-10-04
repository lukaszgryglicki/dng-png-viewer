use crate::view::Key;
use anyhow::Result;

#[cfg(target_os = "freebsd")]
mod freebsd;
#[cfg(target_os = "freebsd")]
pub use freebsd::Console;

#[cfg(not(target_os = "freebsd"))]
pub struct Console;

#[cfg(not(target_os = "freebsd"))]
impl Console {
    pub fn open(_driver: &str) -> Result<Option<Self>> {
        // SDL owns the VT lifecycle on other platforms.
        Ok(None)
    }

    pub fn stopping(&self) -> bool {
        false
    }

    pub fn keys(&mut self) -> Result<Vec<Key>> {
        Ok(Vec::new())
    }

    pub fn finish(self, result: Result<()>) -> Result<()> {
        result
    }
}

#[cfg(any(target_os = "freebsd", test))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "freebsd", test))]
#[derive(Default)]
struct Keyboard {
    sequence: Vec<u8>,
    started: Option<Instant>,
}

#[cfg(any(target_os = "freebsd", test))]
impl Keyboard {
    fn feed(&mut self, bytes: &[u8], now: Instant) -> Vec<Key> {
        let mut keys = self.expire(now);
        for &byte in bytes {
            if self.sequence.is_empty() {
                match byte {
                    0x1b => {
                        self.sequence.push(byte);
                        self.started = Some(now);
                    }
                    b'z' | b'Z' => keys.push(Key::Native),
                    b'x' | b'X' => keys.push(Key::Fit),
                    b'q' | b'Q' | 0x03 | 0x1a | 0x1c => keys.push(Key::Quit),
                    _ => {}
                }
            } else if self.sequence.len() == 1 {
                if byte == b'[' || byte == b'O' {
                    self.sequence.push(byte);
                } else {
                    self.clear();
                    keys.push(Key::Quit);
                }
            } else if (0x40..=0x7e).contains(&byte) {
                match byte {
                    b'A' => keys.push(Key::Up),
                    b'B' => keys.push(Key::Down),
                    b'C' => keys.push(Key::Right),
                    b'D' => keys.push(Key::Left),
                    _ => {}
                }
                self.clear();
            } else if self.sequence.len() < 32 {
                self.sequence.push(byte);
            } else {
                self.clear();
            }
        }
        keys
    }

    fn expire(&mut self, now: Instant) -> Vec<Key> {
        if self
            .started
            .is_some_and(|start| now.duration_since(start) >= Duration::from_millis(50))
        {
            let quit = self.sequence.len() == 1;
            self.clear();
            if quit {
                return vec![Key::Quit];
            }
        }
        Vec::new()
    }

    fn clear(&mut self) {
        self.sequence.clear();
        self.started = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_keys_match_desktop_controls() {
        let mut keyboard = Keyboard::default();
        assert_eq!(
            keyboard.feed(
                b"zZxX\x1b[A\x1b[B\x1b[C\x1b[D\x1bOA\x1bOB\x1bOC\x1bOD",
                Instant::now()
            ),
            [
                Key::Native,
                Key::Native,
                Key::Fit,
                Key::Fit,
                Key::Up,
                Key::Down,
                Key::Right,
                Key::Left,
                Key::Up,
                Key::Down,
                Key::Right,
                Key::Left,
            ]
        );
    }

    #[test]
    fn fragmented_arrows_are_not_mistaken_for_escape() {
        for sequence in [b"\x1b[A".as_slice(), b"\x1bOD", b"\x1b[1;5C"] {
            for split in 1..sequence.len() {
                let now = Instant::now();
                let mut keyboard = Keyboard::default();
                assert!(keyboard.feed(&sequence[..split], now).is_empty());
                let keys = keyboard.feed(&sequence[split..], now + Duration::from_millis(20));
                assert_eq!(keys.len(), 1);
                assert_ne!(keys[0], Key::Quit);
            }
        }
    }

    #[test]
    fn escape_and_interrupt_suspend_keys_exit_cleanly() {
        let now = Instant::now();
        let mut keyboard = Keyboard::default();
        assert!(keyboard.feed(b"\x1b", now).is_empty());
        assert!(keyboard.expire(now + Duration::from_millis(49)).is_empty());
        assert_eq!(
            keyboard.expire(now + Duration::from_millis(50)),
            [Key::Quit]
        );
        assert!(keyboard.expire(now + Duration::from_secs(1)).is_empty());
        assert_eq!(keyboard.feed(b"\x03\x1a\x1c", now), [Key::Quit; 3]);
        assert_eq!(keyboard.feed(b"qQ", now), [Key::Quit; 2]);
        assert_eq!(keyboard.feed(b"\x1b\x1b", now), [Key::Quit]);
    }

    #[test]
    fn unknown_and_incomplete_sequences_do_not_become_viewer_actions() {
        let now = Instant::now();
        let mut keyboard = Keyboard::default();
        assert!(keyboard.feed(b"\x1bOP\x1b[15~abc123", now).is_empty());
        assert!(keyboard.feed(b"\x1b[", now).is_empty());
        assert!(keyboard.expire(now + Duration::from_secs(1)).is_empty());
        assert!(keyboard.feed(&[b'9'; 100], now).is_empty());
        assert_eq!(keyboard.feed(b"x", now), [Key::Fit]);
    }
}
