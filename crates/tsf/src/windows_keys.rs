use windows::Win32::{
    Foundation::{LPARAM, WPARAM},
    UI::Input::KeyboardAndMouse::{
        GetKeyboardLayout, GetKeyboardState, ToUnicodeEx, VIRTUAL_KEY, VK_BACK, VK_CONTROL,
        VK_DOWN, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RETURN, VK_RWIN, VK_SHIFT, VK_SPACE, VK_UP,
    },
};

use crate::keymap::Key;

/// Translate without changing the Windows keyboard buffer. Windows 10 1607
/// and later support the no-state-change flag for ToUnicodeEx.
pub(crate) fn key_from_message(key: WPARAM, message: LPARAM) -> Option<Key> {
    let code = u16::try_from(key.0).ok().filter(|code| *code < 256)?;
    let mut keyboard = [0; 256];
    unsafe { GetKeyboardState(&mut keyboard) }.ok()?;
    let scan = ((message.0 as usize >> 16) & 0xff) as u32;
    classify(code, &keyboard, || {
        translate_character(code, scan, &keyboard)
    })
}

fn classify(
    code: u16,
    keyboard: &[u8; 256],
    character: impl FnOnce() -> Option<Key>,
) -> Option<Key> {
    let pressed = |key: u16| keyboard[usize::from(key)] & 0x80 != 0;
    if [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
        .into_iter()
        .any(|key| pressed(key.0))
    {
        return None;
    }

    let command = match VIRTUAL_KEY(code) {
        VK_SPACE => Some(Key::Space),
        VK_UP => Some(Key::Up),
        VK_DOWN => Some(Key::Down),
        VK_RETURN => Some(Key::Enter),
        VK_ESCAPE => Some(Key::Escape),
        VK_BACK => Some(Key::Backspace),
        _ => None,
    };
    if let Some(command) = command {
        return (!pressed(VK_SHIFT.0)).then_some(command);
    }
    character()
}

fn translate_character(code: u16, scan: u32, keyboard: &[u8; 256]) -> Option<Key> {
    let mut output = [0; 4];
    let count = unsafe {
        ToUnicodeEx(
            u32::from(code),
            scan,
            keyboard,
            &mut output,
            4,
            GetKeyboardLayout(0),
        )
    };
    // Ignore dead keys, sequences, and non-ASCII keyboard output.
    if count == 1 && output[0] < 0x80 {
        Some(Key::Character(char::from(output[0] as u8)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_modifiers_bypass_character_translation_and_commands() {
        for modifier in [VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN] {
            let mut keyboard = [0; 256];
            keyboard[usize::from(modifier.0)] = 0x80;
            for key in [u16::from(b'A'), VK_SPACE.0, VK_BACK.0] {
                assert_eq!(
                    classify(key, &keyboard, || panic!("shortcut translated")),
                    None
                );
            }
        }
    }

    #[test]
    fn shift_can_translate_letters_without_eating_shift_commands() {
        let mut keyboard = [0; 256];
        keyboard[usize::from(VK_SHIFT.0)] = 0x80;
        assert_eq!(
            classify(VK_SPACE.0, &keyboard, || panic!("command translated")),
            None
        );
        assert_eq!(
            classify(u16::from(b'A'), &keyboard, || Some(Key::Character('A'))),
            Some(Key::Character('A'))
        );
    }
}
