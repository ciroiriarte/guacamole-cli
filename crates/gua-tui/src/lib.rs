//! In-terminal raw passthrough for Guacamole text protocols.
//!
//! This first-pass renderer intentionally avoids terminal chrome. It places the
//! local terminal in raw mode, writes bytes from the patched guacd `STDOUT` pipe
//! directly to stdout, and translates local key events into Guacamole `key`
//! press/release instructions.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use gua_core::{Error, Result};
use gua_session::{Session, SessionEvent};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const EXIT_KEYSYM: u32 = 0x1D; // Ctrl-] - common telnet/ssh escape convention.

/// Run the minimal text-mode console loop.
pub fn run_text_session(session: &mut Session) -> Result<()> {
    let _raw = RawModeGuard::enter()?;
    let mut stdout = io::stdout();

    loop {
        if event::poll(POLL_INTERVAL).map_err(|e| Error::Io(io::Error::other(e)))? {
            if let Event::Key(key) = event::read().map_err(|e| Error::Io(io::Error::other(e)))? {
                let keysym = key_to_keysym(key);
                if keysym == Some(EXIT_KEYSYM) {
                    break;
                }
                if let Some(keysym) = keysym {
                    session.send_key(keysym)?;
                }
            }
        }

        if let Some(event) = session.next_event_timeout(POLL_INTERVAL)? {
            match event {
                SessionEvent::StdoutBytes(bytes) => {
                    stdout.write_all(&bytes)?;
                    stdout.flush()?;
                }
                SessionEvent::StdoutEnded | SessionEvent::Disconnected => break,
                SessionEvent::StdoutOpened { .. } | SessionEvent::Ignored(_) => {}
            }
        }
    }

    Ok(())
}

fn key_to_keysym(key: KeyEvent) -> Option<u32> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char(']') => return Some(EXIT_KEYSYM),
            KeyCode::Char(c) if c.is_ascii_alphabetic() => {
                return Some((c.to_ascii_uppercase() as u32) - 0x40)
            }
            KeyCode::Char(' ') => return Some(0),
            _ => {}
        }
    }

    match key.code {
        KeyCode::Backspace => Some(0xFF08),
        KeyCode::Enter => Some(0xFF0D),
        KeyCode::Left => Some(0xFF51),
        KeyCode::Up => Some(0xFF52),
        KeyCode::Right => Some(0xFF53),
        KeyCode::Down => Some(0xFF54),
        KeyCode::Home => Some(0xFF50),
        KeyCode::End => Some(0xFF57),
        KeyCode::PageUp => Some(0xFF55),
        KeyCode::PageDown => Some(0xFF56),
        KeyCode::Tab => Some(0xFF09),
        KeyCode::BackTab => Some(0xFF09),
        KeyCode::Delete => Some(0xFFFF),
        KeyCode::Insert => Some(0xFF63),
        KeyCode::Esc => Some(0xFF1B),
        KeyCode::Char(c) => Some(c as u32),
        KeyCode::F(n) => f_key_to_keysym(n),
        _ => None,
    }
}

fn f_key_to_keysym(n: u8) -> Option<u32> {
    match n {
        1..=12 => Some(0xFFBE + u32::from(n - 1)),
        _ => None,
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(|e| Error::Io(io::Error::other(e)))?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_basic_keys() {
        assert_eq!(
            key_to_keysym(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            Some('a' as u32)
        );
        assert_eq!(
            key_to_keysym(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(0xFF0D)
        );
        assert_eq!(
            key_to_keysym(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(3)
        );
    }
}
