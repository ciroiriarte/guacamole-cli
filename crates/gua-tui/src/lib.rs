//! In-terminal raw passthrough for Guacamole text protocols.
//!
//! This renderer intentionally avoids terminal chrome. It places the local
//! terminal in raw mode, writes bytes from the patched guacd `STDOUT` pipe
//! directly to stdout, and translates local key/paste events into Guacamole
//! keyboard instructions.

#![forbid(unsafe_code)]

use std::io::{self, Write};
use std::panic;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyEventState, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, size as terminal_size};
use gua_core::{Error, Result};
use gua_session::{Session, SessionEvent};
use signal_hook::{consts::SIGINT, flag, SigId};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const GUAC_DPI: u16 = 96;
const CELL_WIDTH_PX: u16 = 9;
const CELL_HEIGHT_PX: u16 = 14;
const XK_SHIFT_L: u32 = 0xFFE1;
const XK_CONTROL_L: u32 = 0xFFE3;
const XK_META_L: u32 = 0xFFE7;
const XK_ALT_L: u32 = 0xFFE9;
const XK_SUPER_L: u32 = 0xFFEB;
const XK_HYPER_L: u32 = 0xFFED;

/// Run the minimal text-mode console loop.
pub fn run_text_session(session: &mut Session) -> Result<()> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let _raw = RawModeGuard::enter(Arc::clone(&interrupted))?;
    let mut stdout = io::stdout();

    let mut last_terminal_size = None;
    maybe_send_terminal_size(session, &mut last_terminal_size)?;

    loop {
        if interrupted.load(Ordering::Relaxed) {
            break;
        }

        maybe_send_terminal_size(session, &mut last_terminal_size)?;

        if event::poll(POLL_INTERVAL).map_err(|e| Error::Io(io::Error::other(e)))? {
            match event::read().map_err(|e| Error::Io(io::Error::other(e)))? {
                Event::Key(key) => match key_to_action(key) {
                    InputAction::Exit => break,
                    InputAction::Ignore => {}
                    InputAction::Key { keysym, modifiers } => {
                        session.send_key_combo(&modifiers, keysym)?;
                    }
                },
                Event::Paste(text) => {
                    for keysym in paste_to_keysyms(&text) {
                        session.send_key(keysym)?;
                    }
                }
                Event::Resize(cols, rows) => {
                    send_terminal_size(session, cols, rows)?;
                    last_terminal_size = Some((cols, rows));
                }
                _ => {}
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

fn maybe_send_terminal_size(
    session: &mut Session,
    last_terminal_size: &mut Option<(u16, u16)>,
) -> Result<()> {
    let Ok((cols, rows)) = terminal_size() else {
        return Ok(());
    };

    if *last_terminal_size == Some((cols, rows)) {
        return Ok(());
    }

    send_terminal_size(session, cols, rows)?;
    *last_terminal_size = Some((cols, rows));
    Ok(())
}

fn send_terminal_size(session: &mut Session, cols: u16, rows: u16) -> Result<()> {
    let (width, height) = terminal_cells_to_pixels(cols, rows);
    session.send_size(width, height, GUAC_DPI)
}

fn terminal_cells_to_pixels(cols: u16, rows: u16) -> (u16, u16) {
    (
        cols.saturating_mul(CELL_WIDTH_PX),
        rows.saturating_mul(CELL_HEIGHT_PX),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum InputAction {
    Exit,
    Ignore,
    Key { keysym: u32, modifiers: Vec<u32> },
}

fn key_to_action(key: KeyEvent) -> InputAction {
    if key.kind == KeyEventKind::Release {
        return InputAction::Ignore;
    }

    if is_local_exit(&key) {
        return InputAction::Exit;
    }

    if let Some(keysym) = control_char_keysym(key) {
        return InputAction::Key {
            keysym,
            modifiers: vec![],
        };
    }

    let Some(keysym) = key_to_keysym(key) else {
        return InputAction::Ignore;
    };

    InputAction::Key {
        keysym,
        modifiers: modifier_keysyms(key),
    }
}

fn is_local_exit(key: &KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(']') | KeyCode::Char('5'))
}

fn control_char_keysym(key: KeyEvent) -> Option<u32> {
    let extra_modifiers =
        KeyModifiers::ALT | KeyModifiers::META | KeyModifiers::SUPER | KeyModifiers::HYPER;
    if !key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.intersects(extra_modifiers) {
        return None;
    }

    match key.code {
        KeyCode::Char(c) if c.is_ascii_alphabetic() => Some((c.to_ascii_uppercase() as u32) - 0x40),
        KeyCode::Char(' ') | KeyCode::Char('2') => Some(0x00),
        KeyCode::Char('[') | KeyCode::Char('3') => Some(0x1B),
        KeyCode::Char('\\') | KeyCode::Char('4') => Some(0x1C),
        KeyCode::Char('^') | KeyCode::Char('6') => Some(0x1E),
        KeyCode::Char('_') | KeyCode::Char('-') | KeyCode::Char('7') => Some(0x1F),
        KeyCode::Char('?') | KeyCode::Char('8') => Some(0x7F),
        _ => None,
    }
}

fn key_to_keysym(key: KeyEvent) -> Option<u32> {
    if key.state.contains(KeyEventState::KEYPAD) {
        if let Some(keysym) = keypad_keysym(key.code) {
            return Some(keysym);
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
        KeyCode::Tab | KeyCode::BackTab => Some(0xFF09),
        KeyCode::Delete => Some(0xFFFF),
        KeyCode::Insert => Some(0xFF63),
        KeyCode::Esc => Some(0xFF1B),
        KeyCode::Null => Some(0),
        KeyCode::Char(c) => Some(c as u32),
        KeyCode::F(n) => f_key_to_keysym(n),
        KeyCode::KeypadBegin => Some(0xFF9D),
        _ => None,
    }
}

fn keypad_keysym(code: KeyCode) -> Option<u32> {
    match code {
        KeyCode::Char(c @ '0'..='9') => Some(0xFFB0 + u32::from(c as u8 - b'0')),
        KeyCode::Char('*') => Some(0xFFAA),
        KeyCode::Char('+') => Some(0xFFAB),
        KeyCode::Char(',') => Some(0xFFAC),
        KeyCode::Char('-') => Some(0xFFAD),
        KeyCode::Char('.') => Some(0xFFAE),
        KeyCode::Char('/') => Some(0xFFAF),
        KeyCode::Char('=') => Some(0xFFBD),
        KeyCode::Enter => Some(0xFF8D),
        KeyCode::Tab | KeyCode::BackTab => Some(0xFF89),
        KeyCode::Left => Some(0xFF96),
        KeyCode::Up => Some(0xFF97),
        KeyCode::Right => Some(0xFF98),
        KeyCode::Down => Some(0xFF99),
        KeyCode::Home => Some(0xFF95),
        KeyCode::PageUp => Some(0xFF9A),
        KeyCode::PageDown => Some(0xFF9B),
        KeyCode::End => Some(0xFF9C),
        KeyCode::KeypadBegin => Some(0xFF9D),
        KeyCode::Insert => Some(0xFF9E),
        KeyCode::Delete => Some(0xFF9F),
        KeyCode::F(1) => Some(0xFF91),
        KeyCode::F(2) => Some(0xFF92),
        KeyCode::F(3) => Some(0xFF93),
        KeyCode::F(4) => Some(0xFF94),
        _ => None,
    }
}

fn f_key_to_keysym(n: u8) -> Option<u32> {
    match n {
        1..=35 => Some(0xFFBE + u32::from(n - 1)),
        _ => None,
    }
}

fn modifier_keysyms(key: KeyEvent) -> Vec<u32> {
    let mut modifiers = Vec::new();
    let printable_char = matches!(key.code, KeyCode::Char(_));

    // Crossterm reports the already-shifted printable character. Wrapping an
    // ordinary printable with Shift would double-apply local layout semantics;
    // keep Shift only for non-printing keys, where guacd needs it for xterm CSI
    // modifier parameters (Shift+Arrow, Shift+F-key, etc.).
    if !printable_char && key.modifiers.contains(KeyModifiers::SHIFT) {
        modifiers.push(XK_SHIFT_L);
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        modifiers.push(XK_CONTROL_L);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        modifiers.push(XK_ALT_L);
    }
    if key.modifiers.contains(KeyModifiers::META) {
        modifiers.push(XK_META_L);
    }
    if key.modifiers.contains(KeyModifiers::SUPER) {
        modifiers.push(XK_SUPER_L);
    }
    if key.modifiers.contains(KeyModifiers::HYPER) {
        modifiers.push(XK_HYPER_L);
    }
    modifiers
}

fn paste_to_keysyms(text: &str) -> Vec<u32> {
    // Deliberately paste literal text as key events. This avoids injecting
    // bracketed-paste delimiters into applications that have not enabled
    // bracketed paste remotely, while still allowing shells/editors to receive
    // exactly the pasted bytes as user input.
    text.chars().map(|c| c as u32).collect()
}

type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

struct RawModeGuard {
    previous_panic_hook: Arc<Mutex<Option<PanicHook>>>,
    sigint_handler: SigId,
}

impl RawModeGuard {
    fn enter(interrupted: Arc<AtomicBool>) -> Result<Self> {
        enable_raw_mode().map_err(|e| Error::Io(io::Error::other(e)))?;
        execute!(io::stdout(), EnableBracketedPaste).map_err(Error::Io)?;
        let sigint_handler =
            flag::register(SIGINT, interrupted).map_err(|e| Error::Io(io::Error::other(e)))?;

        let previous_panic_hook = Arc::new(Mutex::new(Some(panic::take_hook())));
        let hook_previous = Arc::clone(&previous_panic_hook);
        panic::set_hook(Box::new(move |info| {
            restore_terminal_modes();
            if let Ok(previous) = hook_previous.lock() {
                if let Some(previous) = previous.as_ref() {
                    previous(info);
                }
            }
        }));

        Ok(Self {
            previous_panic_hook,
            sigint_handler,
        })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        restore_terminal_modes();
        signal_hook::low_level::unregister(self.sigint_handler);
        if let Ok(mut previous) = self.previous_panic_hook.lock() {
            if let Some(previous) = previous.take() {
                panic::set_hook(previous);
            }
        }
    }
}

fn restore_terminal_modes() {
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    let _ = disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypad(code: KeyCode) -> KeyEvent {
        KeyEvent::new_with_kind_and_state(
            code,
            KeyModifiers::NONE,
            KeyEventKind::Press,
            KeyEventState::KEYPAD,
        )
    }

    #[test]
    fn maps_basic_keys() {
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)),
            InputAction::Key {
                keysym: 'a' as u32,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            InputAction::Key {
                keysym: 0xFF0D,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            InputAction::Key {
                keysym: 3,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('5'), KeyModifiers::CONTROL)),
            InputAction::Exit
        );
    }

    #[test]
    fn maps_plain_ctrl_printables_to_c0_controls() {
        assert_eq!(
            control_char_keysym(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            Some(4)
        );
        assert_eq!(
            control_char_keysym(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            Some(0)
        );
        assert_eq!(
            control_char_keysym(KeyEvent::new(KeyCode::Char('['), KeyModifiers::CONTROL)),
            Some(0x1B)
        );
        assert_eq!(
            control_char_keysym(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::CONTROL)),
            Some(0x7F)
        );
        assert_eq!(
            control_char_keysym(KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )),
            None
        );
    }

    #[test]
    fn maps_navigation_and_function_modifiers() {
        assert_eq!(
            key_to_action(KeyEvent::new(
                KeyCode::Left,
                KeyModifiers::SHIFT | KeyModifiers::ALT
            )),
            InputAction::Key {
                keysym: 0xFF51,
                modifiers: vec![XK_SHIFT_L, XK_ALT_L]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::F(13), KeyModifiers::CONTROL)),
            InputAction::Key {
                keysym: 0xFFCA,
                modifiers: vec![XK_CONTROL_L]
            }
        );
        assert_eq!(
            key_to_keysym(KeyEvent::new(KeyCode::F(35), KeyModifiers::NONE)),
            Some(0xFFE0)
        );
        assert_eq!(
            key_to_keysym(KeyEvent::new(KeyCode::F(36), KeyModifiers::NONE)),
            None
        );
    }

    #[test]
    fn maps_keypad_variants() {
        assert_eq!(key_to_keysym(keypad(KeyCode::Char('7'))), Some(0xFFB7));
        assert_eq!(key_to_keysym(keypad(KeyCode::Enter)), Some(0xFF8D));
        assert_eq!(key_to_keysym(keypad(KeyCode::Left)), Some(0xFF96));
        assert_eq!(key_to_keysym(keypad(KeyCode::Delete)), Some(0xFF9F));
        assert_eq!(key_to_keysym(keypad(KeyCode::Char('/'))), Some(0xFFAF));
    }

    #[test]
    fn ignores_release_events() {
        assert_eq!(
            key_to_action(KeyEvent::new_with_kind(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            )),
            InputAction::Ignore
        );
    }

    #[test]
    fn converts_terminal_cells_to_guacamole_pixels() {
        assert_eq!(terminal_cells_to_pixels(80, 24), (720, 336));
        assert_eq!(
            terminal_cells_to_pixels(u16::MAX, u16::MAX),
            (u16::MAX, u16::MAX)
        );
    }

    #[test]
    fn paste_is_literal_keysyms() {
        assert_eq!(
            paste_to_keysyms("a\nβ"),
            vec!['a' as u32, '\n' as u32, 'β' as u32]
        );
    }
}
