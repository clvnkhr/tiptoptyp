use eframe::egui::{Event, ImeEvent, Key, Modifiers};
use libghostty_vt::key::{Action, Key as GhostKey, Mods};

#[derive(Clone, Debug)]
pub(super) struct KeyInput {
    pub key: GhostKey,
    pub mods: Mods,
    pub action: Action,
    pub unshifted: char,
    pub text: Option<String>,
}

/// Pair egui's physical key event with its text event so Kitty-enabled programs
/// receive one encoded key, and normal shells do not receive each letter twice.
pub(super) fn keys(events: &[Event]) -> Vec<KeyInput> {
    let mut result = Vec::new();
    let mut paired_text = None;
    for (index, event) in events.iter().enumerate() {
        match event {
            Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } => {
                // Cmd shortcuts belong to the host. Raw Control stays in the PTY.
                if modifiers.mac_cmd {
                    continue;
                }
                let text_index = if *pressed {
                    events[index + 1..]
                        .iter()
                        .position(|event| {
                            matches!(event, Event::Text(_) | Event::Key { pressed: true, .. })
                        })
                        .map(|offset| index + 1 + offset)
                } else {
                    None
                };
                let text = text_index.and_then(|index| match &events[index] {
                    Event::Text(text) => {
                        paired_text = Some(index);
                        Some(text.clone())
                    }
                    _ => None,
                });
                let (key, unshifted) = map_key(*key);
                result.push(KeyInput {
                    key,
                    unshifted,
                    text,
                    mods: mods(*modifiers),
                    action: if !pressed {
                        Action::Release
                    } else if *repeat {
                        Action::Repeat
                    } else {
                        Action::Press
                    },
                });
            }
            Event::Text(text) if paired_text != Some(index) => result.push(text_input(text)),
            Event::Ime(ImeEvent::Commit(text)) => result.push(text_input(text)),
            _ => {}
        }
    }
    result
}

fn text_input(text: &str) -> KeyInput {
    KeyInput {
        key: GhostKey::Unidentified,
        unshifted: '\0',
        mods: Mods::empty(),
        action: Action::Press,
        text: Some(text.to_owned()),
    }
}

fn mods(modifiers: Modifiers) -> Mods {
    let mut mods = Mods::empty();
    mods.set(Mods::SHIFT, modifiers.shift);
    mods.set(Mods::ALT, modifiers.alt);
    mods.set(Mods::CTRL, modifiers.ctrl);
    mods
}

fn map_key(key: Key) -> (GhostKey, char) {
    macro_rules! mapping {
        ($($egui:ident => $ghost:ident, $ch:literal;)+) => {
            match key { $(Key::$egui => (GhostKey::$ghost, $ch),)+ _ => (GhostKey::Unidentified, '\0') }
        }
    }
    mapping! {
        A => A, 'a'; B => B, 'b'; C => C, 'c'; D => D, 'd'; E => E, 'e';
        F => F, 'f'; G => G, 'g'; H => H, 'h'; I => I, 'i'; J => J, 'j';
        K => K, 'k'; L => L, 'l'; M => M, 'm'; N => N, 'n'; O => O, 'o';
        P => P, 'p'; Q => Q, 'q'; R => R, 'r'; S => S, 's'; T => T, 't';
        U => U, 'u'; V => V, 'v'; W => W, 'w'; X => X, 'x'; Y => Y, 'y'; Z => Z, 'z';
        Num0 => Digit0, '0'; Num1 => Digit1, '1'; Num2 => Digit2, '2';
        Num3 => Digit3, '3'; Num4 => Digit4, '4'; Num5 => Digit5, '5';
        Num6 => Digit6, '6'; Num7 => Digit7, '7'; Num8 => Digit8, '8'; Num9 => Digit9, '9';
        Backtick => Backquote, '`'; Minus => Minus, '-'; Equals => Equal, '=';
        Plus => Equal, '='; OpenBracket => BracketLeft, '['; CloseBracket => BracketRight, ']';
        OpenCurlyBracket => BracketLeft, '['; CloseCurlyBracket => BracketRight, ']';
        Backslash => Backslash, '\\'; Pipe => Backslash, '\\';
        Semicolon => Semicolon, ';'; Colon => Semicolon, ';'; Quote => Quote, '\'';
        Comma => Comma, ','; Period => Period, '.'; Slash => Slash, '/'; Questionmark => Slash, '/';
        Space => Space, ' '; Enter => Enter, '\r'; Tab => Tab, '\t';
        Escape => Escape, '\0'; Backspace => Backspace, '\0'; Delete => Delete, '\0';
        Insert => Insert, '\0'; Home => Home, '\0'; End => End, '\0';
        PageUp => PageUp, '\0'; PageDown => PageDown, '\0';
        ArrowUp => ArrowUp, '\0'; ArrowDown => ArrowDown, '\0';
        ArrowLeft => ArrowLeft, '\0'; ArrowRight => ArrowRight, '\0';
        F1 => F1, '\0'; F2 => F2, '\0'; F3 => F3, '\0'; F4 => F4, '\0';
        F5 => F5, '\0'; F6 => F6, '\0'; F7 => F7, '\0'; F8 => F8, '\0';
        F9 => F9, '\0'; F10 => F10, '\0'; F11 => F11, '\0'; F12 => F12, '\0';
    }
}

#[cfg(test)]
mod tests {
    use super::super::engine::{Colors, Engine, GridSize};
    use super::*;
    use eframe::egui::Color32;

    fn key(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn text_is_paired_once_and_control_keys_use_ghostty_encoder() {
        let mut engine = Engine::new(
            GridSize {
                cols: 20,
                rows: 5,
                cell_width: 8,
                cell_height: 16,
            },
            Colors {
                foreground: Color32::WHITE,
                background: Color32::BLACK,
            },
        )
        .unwrap();
        let events = [key(Key::A, Modifiers::NONE), Event::Text("a".into())];
        let inputs = keys(&events);
        assert_eq!(inputs.len(), 1);
        assert_eq!(engine.key(inputs[0].clone()).unwrap(), b"a");
        for (key_code, expected) in [
            (Key::C, 3),
            (Key::D, 4),
            (Key::L, 12),
            (Key::R, 18),
            (Key::Z, 26),
        ] {
            assert_eq!(
                engine
                    .key(keys(&[key(key_code, Modifiers::CTRL)])[0].clone())
                    .unwrap(),
                [expected]
            );
        }
        engine.terminal.vt_write(b"\x1b[?1h");
        assert_eq!(
            engine
                .key(keys(&[key(Key::ArrowUp, Modifiers::NONE)])[0].clone())
                .unwrap(),
            b"\x1bOA"
        );
    }

    #[test]
    fn ime_commits_are_forwarded_once_without_preedit() {
        let inputs = keys(&[
            Event::Ime(ImeEvent::Preedit {
                text: "ni".into(),
                active_range_chars: None,
            }),
            Event::Ime(ImeEvent::Commit("你".into())),
        ]);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].text.as_deref(), Some("你"));
    }
}
