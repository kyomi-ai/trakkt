// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyboard input translation for the terminal emulator.
//!
//! Converts browser [`KeyboardEvent`]s into the byte sequences a VT terminal
//! expects, handling printable characters, Ctrl-modified keys, special keys,
//! arrow keys (normal vs application cursor mode), navigation keys, and
//! function keys F1–F12.

use web_sys::KeyboardEvent;

/// Translate a browser keyboard event into the byte sequence that should be
/// sent to the terminal PTY.
///
/// Returns `None` when the key should be ignored or left to the browser's
/// default handler (e.g. Meta/Cmd combos, modifier-only keys).
///
/// Calls [`KeyboardEvent::prevent_default`] for every key we handle, with
/// specific exceptions:
/// - Ctrl+Shift+C (copy) — browser handles it
/// - Ctrl+V (paste) — handled via the paste event path
pub fn translate_key(event: &KeyboardEvent, application_cursor_mode: bool) -> Option<Vec<u8>> {
    let key = event.key();

    // ── Modifier-only keys — always ignore ───────────────────────────────
    match key.as_str() {
        "Shift" | "Control" | "Alt" | "Meta" | "CapsLock" | "NumLock" | "ScrollLock" => {
            return None;
        }
        _ => {}
    }

    // ── Meta / Cmd combos — let the browser handle them ──────────────────
    if event.meta_key() {
        return None;
    }

    let ctrl = event.ctrl_key();
    let alt = event.alt_key();

    // ── Ctrl+Shift+C → copy (browser handles) ───────────────────────────
    if ctrl && event.shift_key() && key == "C" {
        return None;
    }

    // ── Ctrl+V → paste (handled via paste event) ─────────────────────────
    if ctrl && key == "v" {
        return None;
    }

    // ── Ctrl + single letter a-z / A-Z ───────────────────────────────────
    if ctrl && !alt && let Some(byte) = ctrl_key_byte(&key) {
        event.prevent_default();
        return Some(vec![byte]);
    }

    // ── Special keys ─────────────────────────────────────────────────────
    if let Some(bytes) = special_key(&key, application_cursor_mode) {
        event.prevent_default();
        return Some(bytes);
    }

    // ── Alt + printable character ────────────────────────────────────────
    if alt && !ctrl {
        let chars: Vec<char> = key.chars().collect();
        if chars.len() == 1 {
            event.prevent_default();
            let mut bytes = vec![0x1B]; // ESC prefix
            let mut buf = [0u8; 4];
            bytes.extend_from_slice(chars[0].encode_utf8(&mut buf).as_bytes());
            return Some(bytes);
        }
    }

    // ── Printable character (no Ctrl/Alt modifier) ───────────────────────
    if !ctrl && !alt {
        let chars: Vec<char> = key.chars().collect();
        if chars.len() == 1 {
            event.prevent_default();
            let mut buf = [0u8; 4];
            let encoded = chars[0].encode_utf8(&mut buf);
            return Some(encoded.as_bytes().to_vec());
        }
    }

    // ── Unhandled — let the browser do its thing ─────────────────────────
    None
}

/// Convert pasted text into the raw UTF-8 bytes to send to the terminal.
///
/// Bracketed paste wrapping (`\x1b[200~` … `\x1b[201~`) is the caller's
/// responsibility — check [`TerminalModes::bracketed_paste`] and wrap
/// accordingly.
pub fn handle_paste(text: &str) -> Vec<u8> {
    text.as_bytes().to_vec()
}

// ─── Private helpers ─────────────────────────────────────────────────────────

/// Map Ctrl + key to the corresponding control byte.
///
/// Standard mapping: `Ctrl + <letter>` produces `(letter & 0x1F)`.
/// Also handles the special punctuation cases: `[`, `]`, `\`, `@`.
fn ctrl_key_byte(key: &str) -> Option<u8> {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() != 1 {
        return None;
    }

    let ch = chars[0];
    match ch {
        'a'..='z' => Some(ch as u8 & 0x1F),
        'A'..='Z' => Some(ch.to_ascii_lowercase() as u8 & 0x1F),
        '[' => Some(0x1B), // ESC
        ']' => Some(0x1D), // GS
        '\\' => Some(0x1C), // FS
        '@' => Some(0x00), // NUL
        _ => None,
    }
}

/// Map named special keys to their VT byte sequences.
fn special_key(key: &str, app_cursor: bool) -> Option<Vec<u8>> {
    match key {
        // ── Simple keys ──────────────────────────────────────────────────
        "Enter" => Some(vec![0x0D]),
        "Backspace" => Some(vec![0x7F]),
        "Tab" => Some(vec![0x09]),
        "Escape" => Some(vec![0x1B]),

        // ── Editing keys ─────────────────────────────────────────────────
        "Delete" => Some(vec![0x1B, b'[', b'3', b'~']),
        "Insert" => Some(vec![0x1B, b'[', b'2', b'~']),

        // ── Arrow keys (normal: ESC [ X, application: ESC O X) ──────────
        "ArrowUp" => Some(arrow(b'A', app_cursor)),
        "ArrowDown" => Some(arrow(b'B', app_cursor)),
        "ArrowRight" => Some(arrow(b'C', app_cursor)),
        "ArrowLeft" => Some(arrow(b'D', app_cursor)),

        // ── Navigation ───────────────────────────────────────────────────
        "Home" => Some(vec![0x1B, b'[', b'H']),
        "End" => Some(vec![0x1B, b'[', b'F']),
        "PageUp" => Some(vec![0x1B, b'[', b'5', b'~']),
        "PageDown" => Some(vec![0x1B, b'[', b'6', b'~']),

        // ── Function keys (F1-F4 use SS3, F5-F12 use CSI … ~) ───────────
        "F1" => Some(vec![0x1B, b'O', b'P']),
        "F2" => Some(vec![0x1B, b'O', b'Q']),
        "F3" => Some(vec![0x1B, b'O', b'R']),
        "F4" => Some(vec![0x1B, b'O', b'S']),
        "F5" => Some(vec![0x1B, b'[', b'1', b'5', b'~']),
        "F6" => Some(vec![0x1B, b'[', b'1', b'7', b'~']),
        "F7" => Some(vec![0x1B, b'[', b'1', b'8', b'~']),
        "F8" => Some(vec![0x1B, b'[', b'1', b'9', b'~']),
        "F9" => Some(vec![0x1B, b'[', b'2', b'0', b'~']),
        "F10" => Some(vec![0x1B, b'[', b'2', b'1', b'~']),
        "F11" => Some(vec![0x1B, b'[', b'2', b'3', b'~']),
        "F12" => Some(vec![0x1B, b'[', b'2', b'4', b'~']),

        _ => None,
    }
}

/// Build an arrow-key sequence, choosing between normal mode (`ESC [`) and
/// application cursor mode (`ESC O`).
fn arrow(direction: u8, app_cursor: bool) -> Vec<u8> {
    if app_cursor {
        vec![0x1B, b'O', direction]
    } else {
        vec![0x1B, b'[', direction]
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_key_byte_letters() {
        assert_eq!(ctrl_key_byte("c"), Some(0x03));
        assert_eq!(ctrl_key_byte("C"), Some(0x03));
        assert_eq!(ctrl_key_byte("d"), Some(0x04));
        assert_eq!(ctrl_key_byte("l"), Some(0x0C));
        assert_eq!(ctrl_key_byte("z"), Some(0x1A));
        assert_eq!(ctrl_key_byte("a"), Some(0x01));
    }

    #[test]
    fn ctrl_key_byte_special_punctuation() {
        assert_eq!(ctrl_key_byte("["), Some(0x1B));
        assert_eq!(ctrl_key_byte("]"), Some(0x1D));
        assert_eq!(ctrl_key_byte("\\"), Some(0x1C));
        assert_eq!(ctrl_key_byte("@"), Some(0x00));
    }

    #[test]
    fn ctrl_key_byte_non_letter() {
        assert_eq!(ctrl_key_byte("1"), None);
        assert_eq!(ctrl_key_byte("Enter"), None);
    }

    #[test]
    fn special_key_simple() {
        assert_eq!(special_key("Enter", false), Some(vec![0x0D]));
        assert_eq!(special_key("Backspace", false), Some(vec![0x7F]));
        assert_eq!(special_key("Tab", false), Some(vec![0x09]));
        assert_eq!(special_key("Escape", false), Some(vec![0x1B]));
    }

    #[test]
    fn special_key_editing() {
        assert_eq!(
            special_key("Delete", false),
            Some(vec![0x1B, b'[', b'3', b'~'])
        );
        assert_eq!(
            special_key("Insert", false),
            Some(vec![0x1B, b'[', b'2', b'~'])
        );
    }

    #[test]
    fn arrow_keys_normal_mode() {
        assert_eq!(
            special_key("ArrowUp", false),
            Some(vec![0x1B, b'[', b'A'])
        );
        assert_eq!(
            special_key("ArrowDown", false),
            Some(vec![0x1B, b'[', b'B'])
        );
        assert_eq!(
            special_key("ArrowRight", false),
            Some(vec![0x1B, b'[', b'C'])
        );
        assert_eq!(
            special_key("ArrowLeft", false),
            Some(vec![0x1B, b'[', b'D'])
        );
    }

    #[test]
    fn arrow_keys_application_mode() {
        assert_eq!(
            special_key("ArrowUp", true),
            Some(vec![0x1B, b'O', b'A'])
        );
        assert_eq!(
            special_key("ArrowDown", true),
            Some(vec![0x1B, b'O', b'B'])
        );
        assert_eq!(
            special_key("ArrowRight", true),
            Some(vec![0x1B, b'O', b'C'])
        );
        assert_eq!(
            special_key("ArrowLeft", true),
            Some(vec![0x1B, b'O', b'D'])
        );
    }

    #[test]
    fn navigation_keys() {
        assert_eq!(
            special_key("Home", false),
            Some(vec![0x1B, b'[', b'H'])
        );
        assert_eq!(
            special_key("End", false),
            Some(vec![0x1B, b'[', b'F'])
        );
        assert_eq!(
            special_key("PageUp", false),
            Some(vec![0x1B, b'[', b'5', b'~'])
        );
        assert_eq!(
            special_key("PageDown", false),
            Some(vec![0x1B, b'[', b'6', b'~'])
        );
    }

    #[test]
    fn function_keys_f1_f4() {
        assert_eq!(
            special_key("F1", false),
            Some(vec![0x1B, b'O', b'P'])
        );
        assert_eq!(
            special_key("F2", false),
            Some(vec![0x1B, b'O', b'Q'])
        );
        assert_eq!(
            special_key("F3", false),
            Some(vec![0x1B, b'O', b'R'])
        );
        assert_eq!(
            special_key("F4", false),
            Some(vec![0x1B, b'O', b'S'])
        );
    }

    #[test]
    fn function_keys_f5_f12() {
        assert_eq!(
            special_key("F5", false),
            Some(vec![0x1B, b'[', b'1', b'5', b'~'])
        );
        assert_eq!(
            special_key("F6", false),
            Some(vec![0x1B, b'[', b'1', b'7', b'~'])
        );
        assert_eq!(
            special_key("F7", false),
            Some(vec![0x1B, b'[', b'1', b'8', b'~'])
        );
        assert_eq!(
            special_key("F8", false),
            Some(vec![0x1B, b'[', b'1', b'9', b'~'])
        );
        assert_eq!(
            special_key("F9", false),
            Some(vec![0x1B, b'[', b'2', b'0', b'~'])
        );
        assert_eq!(
            special_key("F10", false),
            Some(vec![0x1B, b'[', b'2', b'1', b'~'])
        );
        assert_eq!(
            special_key("F11", false),
            Some(vec![0x1B, b'[', b'2', b'3', b'~'])
        );
        assert_eq!(
            special_key("F12", false),
            Some(vec![0x1B, b'[', b'2', b'4', b'~'])
        );
    }

    #[test]
    fn unknown_key_returns_none() {
        assert_eq!(special_key("Unidentified", false), None);
        assert_eq!(special_key("AudioVolumeUp", false), None);
    }

    #[test]
    fn handle_paste_utf8() {
        let result = handle_paste("hello world");
        assert_eq!(result, b"hello world");
    }

    #[test]
    fn handle_paste_unicode() {
        let result = handle_paste("cafe\u{0301}");
        assert_eq!(result, "cafe\u{0301}".as_bytes());
    }

    #[test]
    fn handle_paste_empty() {
        let result = handle_paste("");
        assert!(result.is_empty());
    }
}
