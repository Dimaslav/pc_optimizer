//! Сериализуемый `Key` — переезд для `winit::keyboard::KeyCode`.
//!
//! `KeyCode` в winit не реализует `serde::{Serialize, Deserialize}`,
//! из-за чего `InputMap` нельзя сохранить в файл. Этот модуль закрывает
//! дыру: `Key` полностью сериализуем и двусторонне маппится в `KeyCode`.
//!
//! # Покрытие
//!
//! Перечислены все клавиши, которые реально нужны для игр: буквы,
//! цифры, F1..F12, стрелки, модификаторы, основные спец-клавиши.
//! Пунктуация (`Minus`, `Equal`, `Backquote`) и numpad не включены:
//! для них можно расширить enum, добавив вариант и строчку в match.
//! Неизвестные `KeyCode` маппятся в `Key::Unknown` и не сохраняются.
//!
//! # Клавиши, привязанные к модификаторам
//!
//! `InputMap` **не** проверяет модификаторы автоматически. Если нужно
//! «Shift+A», придётся сравнивать вручную:
//! ```ignore
//! if input.key_down(KeyCode::ShiftLeft) && input.pressed("some_action") { ... }
//! ```
//! Это осознанно: модификатор — не действие.

use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

/// Сериализуемый аналог `KeyCode`.
///
/// Варианты названы так же, как в winit, для простоты миграции.
/// `Unknown` — заглушка для всего, что не покрыто.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(non_camel_case_types)]
pub enum Key {
    // --- Letters ---
    KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM,
    KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,

    // --- Digits ---
    Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,

    // --- Function keys ---
    F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12,

    // --- Specials ---
    Space, Enter, Escape, Tab, Backspace, Delete, Insert,
    Home, End, PageUp, PageDown,

    // --- Arrows ---
    ArrowUp, ArrowDown, ArrowLeft, ArrowRight,

    // --- Modifiers ---
    ShiftLeft, ShiftRight, ControlLeft, ControlRight, AltLeft, AltRight,

    // --- Punctuation ---
    Backquote, Minus, Equal, BracketLeft, BracketRight, Backslash,
    Semicolon, Quote, Comma, Period, Slash,

    /// Заглушка для `KeyCode`, не входящего в покрытие.
    Unknown,
}

impl Key {
    pub fn from_keycode(kc: KeyCode) -> Self {
        use KeyCode::*;
        match kc {
            KeyA => Key::KeyA, KeyB => Key::KeyB, KeyC => Key::KeyC,
            KeyD => Key::KeyD, KeyE => Key::KeyE, KeyF => Key::KeyF,
            KeyG => Key::KeyG, KeyH => Key::KeyH, KeyI => Key::KeyI,
            KeyJ => Key::KeyJ, KeyK => Key::KeyK, KeyL => Key::KeyL,
            KeyM => Key::KeyM, KeyN => Key::KeyN, KeyO => Key::KeyO,
            KeyP => Key::KeyP, KeyQ => Key::KeyQ, KeyR => Key::KeyR,
            KeyS => Key::KeyS, KeyT => Key::KeyT, KeyU => Key::KeyU,
            KeyV => Key::KeyV, KeyW => Key::KeyW, KeyX => Key::KeyX,
            KeyY => Key::KeyY, KeyZ => Key::KeyZ,

            Digit0 => Key::Digit0, Digit1 => Key::Digit1, Digit2 => Key::Digit2,
            Digit3 => Key::Digit3, Digit4 => Key::Digit4, Digit5 => Key::Digit5,
            Digit6 => Key::Digit6, Digit7 => Key::Digit7, Digit8 => Key::Digit8,
            Digit9 => Key::Digit9,

            F1 => Key::F1, F2 => Key::F2, F3 => Key::F3, F4 => Key::F4,
            F5 => Key::F5, F6 => Key::F6, F7 => Key::F7, F8 => Key::F8,
            F9 => Key::F9, F10 => Key::F10, F11 => Key::F11, F12 => Key::F12,

            Space => Key::Space, Enter => Key::Enter, Escape => Key::Escape,
            Tab => Key::Tab, Backspace => Key::Backspace, Delete => Key::Delete,
            Insert => Key::Insert, Home => Key::Home, End => Key::End,
            PageUp => Key::PageUp, PageDown => Key::PageDown,

            ArrowUp => Key::ArrowUp, ArrowDown => Key::ArrowDown,
            ArrowLeft => Key::ArrowLeft, ArrowRight => Key::ArrowRight,

            ShiftLeft => Key::ShiftLeft, ShiftRight => Key::ShiftRight,
            ControlLeft => Key::ControlLeft, ControlRight => Key::ControlRight,
            AltLeft => Key::AltLeft, AltRight => Key::AltRight,

            Backquote => Key::Backquote, Minus => Key::Minus, Equal => Key::Equal,
            BracketLeft => Key::BracketLeft, BracketRight => Key::BracketRight,
            Backslash => Key::Backslash, Semicolon => Key::Semicolon,
            Quote => Key::Quote, Comma => Key::Comma, Period => Key::Period,
            Slash => Key::Slash,

            _ => Key::Unknown,
        }
    }

    pub fn to_keycode(self) -> Option<KeyCode> {
        use KeyCode::*;
        Some(match self {
            Key::KeyA => KeyA, Key::KeyB => KeyB, Key::KeyC => KeyC,
            Key::KeyD => KeyD, Key::KeyE => KeyE, Key::KeyF => KeyF,
            Key::KeyG => KeyG, Key::KeyH => KeyH, Key::KeyI => KeyI,
            Key::KeyJ => KeyJ, Key::KeyK => KeyK, Key::KeyL => KeyL,
            Key::KeyM => KeyM, Key::KeyN => KeyN, Key::KeyO => KeyO,
            Key::KeyP => KeyP, Key::KeyQ => KeyQ, Key::KeyR => KeyR,
            Key::KeyS => KeyS, Key::KeyT => KeyT, Key::KeyU => KeyU,
            Key::KeyV => KeyV, Key::KeyW => KeyW, Key::KeyX => KeyX,
            Key::KeyY => KeyY, Key::KeyZ => KeyZ,

            Key::Digit0 => Digit0, Key::Digit1 => Digit1, Key::Digit2 => Digit2,
            Key::Digit3 => Digit3, Key::Digit4 => Digit4, Key::Digit5 => Digit5,
            Key::Digit6 => Digit6, Key::Digit7 => Digit7, Key::Digit8 => Digit8,
            Key::Digit9 => Digit9,

            Key::F1 => F1, Key::F2 => F2, Key::F3 => F3, Key::F4 => F4,
            Key::F5 => F5, Key::F6 => F6, Key::F7 => F7, Key::F8 => F8,
            Key::F9 => F9, Key::F10 => F10, Key::F11 => F11, Key::F12 => F12,

            Key::Space => Space, Key::Enter => Enter, Key::Escape => Escape,
            Key::Tab => Tab, Key::Backspace => Backspace, Key::Delete => Delete,
            Key::Insert => Insert, Key::Home => Home, Key::End => End,
            Key::PageUp => PageUp, Key::PageDown => PageDown,

            Key::ArrowUp => ArrowUp, Key::ArrowDown => ArrowDown,
            Key::ArrowLeft => ArrowLeft, Key::ArrowRight => ArrowRight,

            Key::ShiftLeft => ShiftLeft, Key::ShiftRight => ShiftRight,
            Key::ControlLeft => ControlLeft, Key::ControlRight => ControlRight,
            Key::AltLeft => AltLeft, Key::AltRight => AltRight,

            Key::Backquote => Backquote, Key::Minus => Minus, Key::Equal => Equal,
            Key::BracketLeft => BracketLeft, Key::BracketRight => BracketRight,
            Key::Backslash => Backslash, Key::Semicolon => Semicolon,
            Key::Quote => Quote, Key::Comma => Comma, Key::Period => Period,
            Key::Slash => Slash,

            Key::Unknown => return None,
        })
    }

    /// Человекочитаемое имя для UI.
    pub fn display_name(self) -> &'static str {
        match self {
            Key::KeyA => "A", Key::KeyB => "B", Key::KeyC => "C", Key::KeyD => "D",
            Key::KeyE => "E", Key::KeyF => "F", Key::KeyG => "G", Key::KeyH => "H",
            Key::KeyI => "I", Key::KeyJ => "J", Key::KeyK => "K", Key::KeyL => "L",
            Key::KeyM => "M", Key::KeyN => "N", Key::KeyO => "O", Key::KeyP => "P",
            Key::KeyQ => "Q", Key::KeyR => "R", Key::KeyS => "S", Key::KeyT => "T",
            Key::KeyU => "U", Key::KeyV => "V", Key::KeyW => "W", Key::KeyX => "X",
            Key::KeyY => "Y", Key::KeyZ => "Z",
            Key::Digit0 => "0", Key::Digit1 => "1", Key::Digit2 => "2",
            Key::Digit3 => "3", Key::Digit4 => "4", Key::Digit5 => "5",
            Key::Digit6 => "6", Key::Digit7 => "7", Key::Digit8 => "8",
            Key::Digit9 => "9",
            Key::F1 => "F1", Key::F2 => "F2", Key::F3 => "F3", Key::F4 => "F4",
            Key::F5 => "F5", Key::F6 => "F6", Key::F7 => "F7", Key::F8 => "F8",
            Key::F9 => "F9", Key::F10 => "F10", Key::F11 => "F11", Key::F12 => "F12",
            Key::Space => "Space", Key::Enter => "Enter", Key::Escape => "Esc",
            Key::Tab => "Tab", Key::Backspace => "Backspace", Key::Delete => "Del",
            Key::Insert => "Ins", Key::Home => "Home", Key::End => "End",
            Key::PageUp => "PgUp", Key::PageDown => "PgDn",
            Key::ArrowUp => "↑", Key::ArrowDown => "↓",
            Key::ArrowLeft => "←", Key::ArrowRight => "→",
            Key::ShiftLeft => "LShift", Key::ShiftRight => "RShift",
            Key::ControlLeft => "LCtrl", Key::ControlRight => "RCtrl",
            Key::AltLeft => "LAlt", Key::AltRight => "RAlt",
            Key::Backquote => "`", Key::Minus => "-", Key::Equal => "=",
            Key::BracketLeft => "[", Key::BracketRight => "]",
            Key::Backslash => "\\", Key::Semicolon => ";", Key::Quote => "'",
            Key::Comma => ",", Key::Period => ".", Key::Slash => "/",
            Key::Unknown => "?",
        }
    }
}

impl Default for Key {
    fn default() -> Self { Self::Unknown }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_keycode() {
        for kc in [
            KeyCode::KeyW, KeyCode::Space, KeyCode::Escape,
            KeyCode::Digit3, KeyCode::F5, KeyCode::ShiftLeft,
        ] {
            let k = Key::from_keycode(kc);
            assert_eq!(k.to_keycode(), Some(kc), "roundtrip failed for {:?}", kc);
        }
    }

    #[test]
    fn unknown_keycode() {
        // Numpad5 точно не в списке.
        let k = Key::from_keycode(KeyCode::Numpad5);
        assert_eq!(k, Key::Unknown);
        assert_eq!(k.to_keycode(), None);
    }
}