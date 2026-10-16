//! Shared single-line text editing used by the create form and the
//! environments & credentials managers: insert-at-cursor semantics
//! (chars, Backspace, Delete, ←/→, Home/End) instead of append-only, so
//! pasted strings can be edited where they were pasted.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Cursor position for a fresh buffer: at the end.
pub fn caret_end(buf: &str) -> usize {
    buf.chars().count()
}

/// Shared editing key route. `Ok(true)` = the key was consumed by editing
/// (let the caller return whatever re-renders); `Ok(false)` = not an
/// editing key — the caller's own match (Enter, Up/Down, …) continues.
pub fn text_edit(buf: &mut String, cursor: &mut usize, key: &KeyEvent) -> bool {
    match key.code {
        KeyCode::Char(c)
            if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT) =>
        {
            // Insert at the caret (pastes arrive as a char stream).
            let at = cursor.clamp_chars(buf);
            let byte = buf
                .char_indices()
                .nth(at)
                .map(|(i, _)| i)
                .unwrap_or(buf.len());
            buf.insert(byte, c);
            *cursor = at + 1;
            true
        }
        KeyCode::Backspace => {
            let at = cursor.clamp_chars(buf);
            if at > 0 {
                let byte = buf
                    .char_indices()
                    .nth(at - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(buf.len());
                buf.remove(byte);
                *cursor = at - 1;
            }
            true
        }
        KeyCode::Delete => {
            let at = cursor.clamp_chars(buf);
            if at < buf.chars().count() {
                let byte = buf
                    .char_indices()
                    .nth(at)
                    .map(|(i, _)| i)
                    .unwrap_or(buf.len());
                buf.remove(byte);
            }
            true
        }
        KeyCode::Left => {
            *cursor = cursor.saturating_sub(1).min(buf.chars().count());
            true
        }
        KeyCode::Right => {
            *cursor = (*cursor + 1).min(buf.chars().count());
            true
        }
        KeyCode::Home => {
            *cursor = 0;
            true
        }
        KeyCode::End => {
            *cursor = buf.chars().count();
            true
        }
        _ => false,
    }
}

/// Split a buffer at the caret into `(before, char-at-caret, after)` for
/// rendering, so the edit point is visible mid-string.
pub fn split_caret(value: &str, cursor: usize) -> (String, Option<char>, String) {
    let chars: Vec<char> = value.chars().collect();
    let at = cursor.min(chars.len());
    let after = if at == chars.len() {
        String::new()
    } else {
        chars[at + 1..].iter().collect()
    };
    (chars[..at].iter().collect(), chars.get(at).copied(), after)
}

/// Clamp a cursor to a buffer's char count (guards stale cursors after
/// external changes to the buffer).
trait ClampChars {
    fn clamp_chars(&self, buf: &str) -> usize;
}

impl ClampChars for usize {
    fn clamp_chars(&self, buf: &str) -> usize {
        (*self).min(buf.chars().count())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn chars_insert_at_the_caret_not_at_the_end() {
        let mut buf = "ac".to_string();
        let mut cursor = 1;
        assert!(text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::Char('b'), KeyModifiers::NONE)
        ));
        assert_eq!(buf, "abc");
        assert_eq!(cursor, 2);
    }

    #[test]
    fn backspace_and_delete_edit_around_the_caret() {
        let mut buf = "abcd".to_string();
        let mut cursor = 2;
        assert!(text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::Backspace, KeyModifiers::NONE)
        ));
        assert_eq!(buf, "acd");
        assert_eq!(cursor, 1);
        assert!(text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::Delete, KeyModifiers::NONE)
        ));
        assert_eq!(buf, "ad");
        assert_eq!(cursor, 1);
    }

    #[test]
    fn arrows_home_end_move_the_caret_without_touching_the_text() {
        let mut buf = "hello".to_string();
        let mut cursor = 4;
        text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::Home, KeyModifiers::NONE),
        );
        assert_eq!(cursor, 0);
        text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::End, KeyModifiers::NONE),
        );
        assert_eq!(cursor, 5);
        text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::Right, KeyModifiers::NONE),
        );
        assert_eq!(cursor, 5, "stays clamped at the end");
    }

    #[test]
    fn paste_is_a_char_stream_inserted_at_the_caret() {
        let mut buf = "ab".to_string();
        let mut cursor = 2;
        for c in "cdef".chars() {
            text_edit(
                &mut buf,
                &mut cursor,
                &key(KeyCode::Char(c), KeyModifiers::NONE),
            );
        }
        assert_eq!(buf, "abcdef");
        assert_eq!(cursor, 6);
    }

    #[test]
    fn caret_split_works_with_multibyte_chars() {
        let (before, caret, after) = split_caret("hülle", 2);
        assert_eq!(before, "hü");
        assert_eq!(caret, Some('l'));
        assert_eq!(after, "le");
        let (before, caret, after) = split_caret("hülle", 5);
        assert_eq!(before, "hülle");
        assert_eq!(caret, None);
        assert_eq!(after, "");
    }

    #[test]
    fn stale_cursor_is_clamped_not_panic() {
        let mut buf = String::from("x");
        let mut cursor = 99;
        text_edit(
            &mut buf,
            &mut cursor,
            &key(KeyCode::Char('y'), KeyModifiers::NONE),
        );
        assert_eq!(buf, "xy");
        assert_eq!(cursor, 2);
    }
}
