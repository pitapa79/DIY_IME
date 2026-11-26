use unicode_segmentation::UnicodeSegmentation as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Space,
    Up,
    Down,
    Enter,
    Escape,
    Backspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Append(char),
    ReplaceWithPrefix(usize),
    Convert,
    NextCandidate,
    PreviousCandidate,
    Commit,
    ReturnToPreedit,
    Cancel,
}

/// A view of the trusted input session at the time of the key event.
pub struct Context<'a> {
    pub editable: bool,
    pub preedit: &'a str,
    pub selecting_candidate: bool,
}

/// Classifies a key without changing the document, input, or conversion state.
/// The caller obtains a fresh view before applying an action on key-down.
pub fn action(key: Key, context: Context<'_>) -> Option<Action> {
    if !context.editable {
        return None;
    }

    match key {
        Key::Character(character) if character.is_ascii_alphabetic() || character == '\'' => {
            return Some(Action::Append(character));
        }
        Key::Character(_) => return None,
        _ if context.preedit.is_empty() => return None,
        _ => {}
    }

    match key {
        Key::Space if context.selecting_candidate => Some(Action::NextCandidate),
        Key::Space => Some(Action::Convert),
        Key::Up if context.selecting_candidate => Some(Action::PreviousCandidate),
        Key::Down if context.selecting_candidate => Some(Action::NextCandidate),
        Key::Enter => Some(Action::Commit),
        Key::Escape if context.selecting_candidate => Some(Action::ReturnToPreedit),
        Key::Escape => Some(Action::Cancel),
        Key::Backspace => context
            .preedit
            .grapheme_indices(true)
            .next_back()
            .map(|(start, _)| Action::ReplaceWithPrefix(start)),
        Key::Up | Key::Down | Key::Character(_) => None,
    }
}
