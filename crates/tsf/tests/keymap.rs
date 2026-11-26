#![cfg(all(windows, target_arch = "x86_64"))]

use diy_ime2_tsf::keymap::{Action, Context, Key, action};

fn editable(key: Key, preedit: &str, selecting_candidate: bool) -> Option<Action> {
    action(
        key,
        Context {
            editable: true,
            preedit,
            selecting_candidate,
        },
    )
}

#[test]
fn shortcuts_and_commands_without_preedit_stay_with_the_application() {
    for key in [
        Key::Space,
        Key::Enter,
        Key::Escape,
        Key::Backspace,
        Key::Up,
        Key::Down,
    ] {
        assert_eq!(editable(key, "", false), None, "{key:?}");
    }
    for key in [
        Key::Character('1'),
        Key::Character('あ'),
        Key::Character('\t'),
    ] {
        assert_eq!(editable(key, "ka", false), None, "{key:?}");
    }
    assert_eq!(
        editable(Key::Character('A'), "", false),
        Some(Action::Append('A'))
    );
    assert_eq!(
        editable(Key::Character('\''), "n", false),
        Some(Action::Append('\''))
    );
}

#[test]
fn composition_and_candidate_keys_use_the_current_phase() {
    assert_eq!(editable(Key::Space, "ka", false), Some(Action::Convert));
    assert_eq!(
        editable(Key::Space, "ka", true),
        Some(Action::NextCandidate)
    );
    assert_eq!(
        editable(Key::Up, "ka", true),
        Some(Action::PreviousCandidate)
    );
    assert_eq!(editable(Key::Down, "ka", true), Some(Action::NextCandidate));
    assert_eq!(editable(Key::Enter, "ka", true), Some(Action::Commit));
    assert_eq!(editable(Key::Enter, "ka", false), Some(Action::Commit));
    assert_eq!(
        editable(Key::Escape, "ka", true),
        Some(Action::ReturnToPreedit)
    );
    assert_eq!(editable(Key::Escape, "ka", false), Some(Action::Cancel));
    assert_eq!(editable(Key::Up, "ka", false), None);
    assert_eq!(editable(Key::Down, "ka", false), None);
}

#[test]
fn inspection_does_not_mutate_state_and_disabled_keys_always_pass() {
    for key in [
        Key::Character('a'),
        Key::Space,
        Key::Enter,
        Key::Escape,
        Key::Backspace,
        Key::Up,
        Key::Down,
    ] {
        assert_eq!(
            action(
                key,
                Context {
                    editable: false,
                    preedit: "n",
                    selecting_candidate: true
                }
            ),
            None
        );
    }
    let preedit = String::from("k");
    assert_eq!(
        editable(Key::Character('a'), &preedit, false),
        Some(Action::Append('a'))
    );
    assert_eq!(
        editable(Key::Character('a'), &preedit, false),
        Some(Action::Append('a'))
    );
    assert_eq!(preedit, "k");
    assert_eq!(
        editable(Key::Character('a'), &preedit, false),
        Some(Action::Append('a'))
    );
}

#[test]
fn backspace_returns_a_utf8_boundary_before_the_last_grapheme() {
    for cluster in ["か\u{3099}", "👩‍💻", "🇯🇵", "𠮷", "a"] {
        let preedit = format!("前{cluster}");
        let Some(Action::ReplaceWithPrefix(boundary)) = editable(Key::Backspace, &preedit, false)
        else {
            panic!("expected a grapheme boundary");
        };
        assert_eq!(&preedit[..boundary], "前", "{cluster}");
    }
    assert_eq!(
        editable(Key::Backspace, "か\u{3099}", false),
        Some(Action::ReplaceWithPrefix(0))
    );
}
