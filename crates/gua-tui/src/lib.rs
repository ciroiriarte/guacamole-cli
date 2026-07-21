//! In-terminal raw passthrough for Guacamole text protocols.
//!
//! This renderer intentionally avoids terminal chrome. It places the local
//! terminal in raw mode, writes bytes from the patched guacd `STDOUT` pipe
//! directly to stdout, and translates local key/paste events into Guacamole
//! keyboard instructions.

#![forbid(unsafe_code)]

use std::io::{self, ErrorKind, Write};
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
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, size as terminal_size, window_size, WindowSize,
};
use gua_core::{Error, Result};
use gua_session::{Session, SessionEvent};
use signal_hook::{
    consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM},
    flag, SigId,
};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const DRAIN_INTERVAL: Duration = Duration::from_millis(1);
const PASTE_DRAIN_CHARS: usize = 256;
const DEFAULT_GUAC_DPI: u32 = 96;
const DEFAULT_CELL_WIDTH_PX: u32 = 9;
const DEFAULT_CELL_HEIGHT_PX: u32 = 14;
const XK_SHIFT_L: u32 = 0xFFE1;
const XK_CONTROL_L: u32 = 0xFFE3;
const XK_META_L: u32 = 0xFFE7;
const XK_ALT_L: u32 = 0xFFE9;
const XK_SUPER_L: u32 = 0xFFEB;
const XK_HYPER_L: u32 = 0xFFED;
const XK_RETURN: u32 = 0xFF0D;
const XK_TAB: u32 = 0xFF09;
const XK_BACK_TAB: u32 = 0xFE20;
const UNICODE_KEYSYM_MASK: u32 = 0x0100_0000;

/// Run the minimal text-mode console loop.
pub fn run_text_session(session: &mut Session) -> Result<()> {
    let interrupted = Arc::new(AtomicBool::new(false));
    let _raw = RawModeGuard::enter(Arc::clone(&interrupted))?;
    let mut stdout = io::stdout();
    let geometry = TerminalGeometry::from_env();

    let mut last_terminal_size = None;
    maybe_send_terminal_size(session, &geometry, &mut last_terminal_size)?;

    loop {
        if interrupted.load(Ordering::Relaxed) {
            break;
        }

        maybe_send_terminal_size(session, &geometry, &mut last_terminal_size)?;

        match poll_input(POLL_INTERVAL) {
            Ok(true) => match read_input_event() {
                Ok(Some(Event::Key(key))) => match key_to_action(key) {
                    InputAction::Exit => break,
                    InputAction::Ignore => {}
                    InputAction::Key { keysym, modifiers } => {
                        session.send_key_combo(&modifiers, keysym)?;
                    }
                },
                Ok(Some(Event::Paste(text))) => {
                    send_paste(session, &text, &mut stdout)?;
                }
                Ok(Some(Event::Resize(cols, rows))) => {
                    send_terminal_size(session, &geometry, cols, rows)?;
                    last_terminal_size = Some((cols, rows));
                }
                Ok(Some(_)) | Ok(None) => {}
                Err(e) => return Err(e),
            },
            Ok(false) => {}
            Err(e) => return Err(e),
        }

        if !drain_session_event(session, &mut stdout, POLL_INTERVAL)? {
            break;
        }
    }

    Ok(())
}

fn poll_input(timeout: Duration) -> Result<bool> {
    match event::poll(timeout) {
        Ok(ready) => Ok(ready),
        Err(e) if e.kind() == ErrorKind::Interrupted => Ok(false),
        Err(e) => Err(Error::Io(io::Error::other(e))),
    }
}

fn read_input_event() -> Result<Option<Event>> {
    match event::read() {
        Ok(event) => Ok(Some(event)),
        Err(e) if e.kind() == ErrorKind::Interrupted => Ok(None),
        Err(e) => Err(Error::Io(io::Error::other(e))),
    }
}

fn drain_session_event(
    session: &mut Session,
    stdout: &mut impl Write,
    timeout: Duration,
) -> Result<bool> {
    let Some(event) = session.next_event_timeout(timeout)? else {
        return Ok(true);
    };
    handle_session_event(stdout, event)
}

fn handle_session_event(stdout: &mut impl Write, event: SessionEvent) -> Result<bool> {
    match event {
        SessionEvent::StdoutBytes(bytes) => {
            stdout.write_all(&bytes)?;
            stdout.flush()?;
            Ok(true)
        }
        SessionEvent::StdoutEnded | SessionEvent::Disconnected => Ok(false),
        SessionEvent::StdoutOpened { .. } | SessionEvent::Ignored(_) => Ok(true),
    }
}

fn send_paste(session: &mut Session, text: &str, stdout: &mut impl Write) -> Result<()> {
    for (idx, keysym) in text.chars().map(paste_char_to_keysym).enumerate() {
        session.send_key(keysym)?;
        if (idx + 1) % PASTE_DRAIN_CHARS == 0
            && !drain_session_event(session, stdout, DRAIN_INTERVAL)?
        {
            break;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TerminalGeometry {
    cell_width_px: u32,
    cell_height_px: u32,
    dpi: u32,
}

impl TerminalGeometry {
    fn from_env() -> Self {
        Self {
            cell_width_px: env_u32("GUA_TUI_CELL_WIDTH_PX", DEFAULT_CELL_WIDTH_PX),
            cell_height_px: env_u32("GUA_TUI_CELL_HEIGHT_PX", DEFAULT_CELL_HEIGHT_PX),
            dpi: env_u32("GUA_TUI_DPI", DEFAULT_GUAC_DPI),
        }
    }
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn maybe_send_terminal_size(
    session: &mut Session,
    geometry: &TerminalGeometry,
    last_terminal_size: &mut Option<(u16, u16)>,
) -> Result<()> {
    let Ok((cols, rows)) = terminal_size() else {
        return Ok(());
    };

    if *last_terminal_size == Some((cols, rows)) {
        return Ok(());
    }

    send_terminal_size(session, geometry, cols, rows)?;
    *last_terminal_size = Some((cols, rows));
    Ok(())
}

fn send_terminal_size(
    session: &mut Session,
    geometry: &TerminalGeometry,
    cols: u16,
    rows: u16,
) -> Result<()> {
    let (width, height) = guacamole_pixel_size(cols, rows, geometry);
    session.send_size(width, height, geometry.dpi)
}

fn guacamole_pixel_size(cols: u16, rows: u16, geometry: &TerminalGeometry) -> (u32, u32) {
    match window_size() {
        Ok(size) => guacamole_pixel_size_from_window(size, geometry),
        Err(_) => terminal_cells_to_pixels(cols, rows, geometry),
    }
}

fn guacamole_pixel_size_from_window(size: WindowSize, geometry: &TerminalGeometry) -> (u32, u32) {
    if size.width > 0 && size.height > 0 {
        (u32::from(size.width), u32::from(size.height))
    } else {
        terminal_cells_to_pixels(size.columns, size.rows, geometry)
    }
}

fn terminal_cells_to_pixels(cols: u16, rows: u16, geometry: &TerminalGeometry) -> (u32, u32) {
    (
        u32::from(cols).saturating_mul(geometry.cell_width_px),
        u32::from(rows).saturating_mul(geometry.cell_height_px),
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

    if let Some(keysym) = text_control_char_keysym(key) {
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

fn text_control_char_keysym(key: KeyEvent) -> Option<u32> {
    let extra_modifiers = KeyModifiers::SHIFT
        | KeyModifiers::ALT
        | KeyModifiers::META
        | KeyModifiers::SUPER
        | KeyModifiers::HYPER;
    if !key.modifiers.contains(KeyModifiers::CONTROL) || key.modifiers.intersects(extra_modifiers) {
        return None;
    }

    // This TUI is currently the raw text-console renderer for SSH/telnet/k8s.
    // For plain Ctrl chords, the remote PTY behavior users expect is the C0 byte
    // itself (Ctrl-C => ETX, Ctrl-D => EOT, Ctrl-A => SOH). The graphical input
    // path must use X11 keysyms + modifier state instead; do not share this
    // text-console compatibility mapping with RDP/VNC/GUI input.
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
        KeyCode::Enter => Some(XK_RETURN),
        KeyCode::Left => Some(0xFF51),
        KeyCode::Up => Some(0xFF52),
        KeyCode::Right => Some(0xFF53),
        KeyCode::Down => Some(0xFF54),
        KeyCode::Home => Some(0xFF50),
        KeyCode::End => Some(0xFF57),
        KeyCode::PageUp => Some(0xFF55),
        KeyCode::PageDown => Some(0xFF56),
        KeyCode::Tab => Some(XK_TAB),
        KeyCode::BackTab => Some(XK_BACK_TAB),
        KeyCode::Delete => Some(0xFFFF),
        KeyCode::Insert => Some(0xFF63),
        KeyCode::Esc => Some(0xFF1B),
        KeyCode::Null => Some(0),
        KeyCode::Char(c) => Some(char_to_keysym(c)),
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
    let non_shift_modifiers = KeyModifiers::CONTROL
        | KeyModifiers::ALT
        | KeyModifiers::META
        | KeyModifiers::SUPER
        | KeyModifiers::HYPER;

    // Crossterm reports ordinary shifted printable characters as their final
    // local-layout glyph (`A`, `!`, etc.). Sending Shift around those glyphs can
    // double-apply layout semantics in guacd. Preserve Shift for non-printing
    // keys and for modified printable chords such as Ctrl+Shift+F or Alt+Shift+X,
    // where remote applications distinguish the modifier state.
    if key.modifiers.contains(KeyModifiers::SHIFT)
        && (!printable_char || key.modifiers.intersects(non_shift_modifiers))
    {
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

#[cfg(test)]
fn paste_to_keysyms(text: &str) -> Vec<u32> {
    // Deliberately paste text as key events. This avoids injecting remote
    // bracketed-paste delimiters into applications that have not enabled
    // bracketed paste, while still allowing shells/editors to receive user input.
    text.chars().map(paste_char_to_keysym).collect()
}

fn paste_char_to_keysym(c: char) -> u32 {
    match c {
        '\n' | '\r' => XK_RETURN,
        _ => char_to_keysym(c),
    }
}

fn char_to_keysym(c: char) -> u32 {
    let codepoint = c as u32;
    if codepoint >= 0x0100 {
        UNICODE_KEYSYM_MASK | codepoint
    } else {
        codepoint
    }
}

type PanicHook = Box<dyn Fn(&panic::PanicHookInfo<'_>) + Sync + Send + 'static>;

struct RawModeGuard {
    previous_panic_hook: Arc<Mutex<Option<PanicHook>>>,
    signal_handlers: Vec<SigId>,
}

impl RawModeGuard {
    fn enter(interrupted: Arc<AtomicBool>) -> Result<Self> {
        enable_raw_mode().map_err(|e| Error::Io(io::Error::other(e)))?;

        match Self::enter_after_raw_mode(interrupted) {
            Ok(guard) => Ok(guard),
            Err(error) => {
                restore_terminal_modes();
                Err(error)
            }
        }
    }

    fn enter_after_raw_mode(interrupted: Arc<AtomicBool>) -> Result<Self> {
        execute!(io::stdout(), EnableBracketedPaste).map_err(Error::Io)?;
        let signal_handlers = register_signal_handlers(interrupted)?;

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
            signal_handlers,
        })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        restore_terminal_modes();
        for handler in self.signal_handlers.drain(..) {
            signal_hook::low_level::unregister(handler);
        }
        if let Ok(mut previous) = self.previous_panic_hook.lock() {
            if let Some(previous) = previous.take() {
                panic::set_hook(previous);
            }
        }
    }
}

fn register_signal_handlers(interrupted: Arc<AtomicBool>) -> Result<Vec<SigId>> {
    let mut handlers = Vec::new();
    for signal in [SIGINT, SIGTERM, SIGHUP, SIGQUIT] {
        match flag::register(signal, Arc::clone(&interrupted)) {
            Ok(handler) => handlers.push(handler),
            Err(error) => {
                for handler in handlers {
                    signal_hook::low_level::unregister(handler);
                }
                return Err(Error::Io(io::Error::other(error)));
            }
        }
    }
    Ok(handlers)
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
                keysym: XK_RETURN,
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
    fn maps_plain_ctrl_printables_to_text_c0_controls() {
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)),
            InputAction::Key {
                keysym: 4,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL)),
            InputAction::Key {
                keysym: 0,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('['), KeyModifiers::CONTROL)),
            InputAction::Key {
                keysym: 0x1B,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(
                KeyCode::Char('d'),
                KeyModifiers::CONTROL | KeyModifiers::ALT
            )),
            InputAction::Key {
                keysym: 'd' as u32,
                modifiers: vec![XK_CONTROL_L, XK_ALT_L]
            }
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
    fn preserves_shift_for_modified_printables() {
        assert_eq!(
            key_to_action(KeyEvent::new(
                KeyCode::Char('F'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )),
            InputAction::Key {
                keysym: 'F' as u32,
                modifiers: vec![XK_SHIFT_L, XK_CONTROL_L]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT)),
            InputAction::Key {
                keysym: 'A' as u32,
                modifiers: vec![]
            }
        );
    }

    #[test]
    fn maps_backtab_distinctly_from_tab() {
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE)),
            InputAction::Key {
                keysym: XK_BACK_TAB,
                modifiers: vec![]
            }
        );
        assert_eq!(
            key_to_action(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            InputAction::Key {
                keysym: XK_TAB,
                modifiers: vec![]
            }
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
        let geometry = TerminalGeometry {
            cell_width_px: 9,
            cell_height_px: 14,
            dpi: DEFAULT_GUAC_DPI,
        };
        assert_eq!(terminal_cells_to_pixels(80, 24, &geometry), (720, 336));
        assert_eq!(
            terminal_cells_to_pixels(u16::MAX, u16::MAX, &geometry),
            (589_815, 917_490)
        );
    }

    #[test]
    fn uses_window_pixels_when_available() {
        let geometry = TerminalGeometry {
            cell_width_px: 9,
            cell_height_px: 14,
            dpi: DEFAULT_GUAC_DPI,
        };
        assert_eq!(
            guacamole_pixel_size_from_window(
                WindowSize {
                    rows: 24,
                    columns: 80,
                    width: 1024,
                    height: 768,
                },
                &geometry,
            ),
            (1024, 768)
        );
        assert_eq!(
            guacamole_pixel_size_from_window(
                WindowSize {
                    rows: 24,
                    columns: 80,
                    width: 0,
                    height: 0,
                },
                &geometry,
            ),
            (720, 336)
        );
    }

    #[test]
    fn paste_maps_newlines_and_unicode_to_x11_keysyms() {
        assert_eq!(
            paste_to_keysyms("a\néβ😀"),
            vec![
                'a' as u32,
                XK_RETURN,
                'é' as u32,
                UNICODE_KEYSYM_MASK | ('β' as u32),
                UNICODE_KEYSYM_MASK | ('😀' as u32),
            ]
        );
    }
}
