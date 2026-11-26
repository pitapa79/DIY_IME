use std::{
    env,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use diy_ime2_ime_host::{
    Conversion,
    external::{Background, Configuration, Event},
};
use diy_ime2_runtime::node::{InputMode, RuntimeInput};

fn wait(background: &Background, mut event: impl FnMut(Event) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if let Some(next) = background.poll().expect("Host connection ended") {
            assert!(!matches!(next, Event::Unavailable), "Host unavailable");
            if event(next) {
                return;
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("Host timed out");
}

struct Reaped(std::sync::mpsc::SyncSender<()>);

impl Drop for Reaped {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

fn main() {
    let (closed, completion) = std::sync::mpsc::sync_channel(1);
    let mut paths = env::args_os().skip(1).map(PathBuf::from);
    let background = Background::spawn(
        Configuration {
            executable: paths.next().expect("Host executable"),
            file: paths.next().expect("Host configuration"),
        },
        Reaped(closed),
    )
    .expect("Host background thread");
    assert!(paths.next().is_none());
    wait(&background, |event| matches!(event, Event::Ready(_)));

    for (revision, text, mode, expected) in [
        (1, "kousui", InputMode::Compose, "降水"),
        (2, "n", InputMode::Finalize, "ん"),
        (3, "", InputMode::Compose, ""),
    ] {
        assert!(background.submit(RuntimeInput {
            revision,
            text: text.into(),
            mode
        }));
        wait(&background, |event| {
            if let Event::Converted(key, result) = event {
                assert_eq!(key.input_revision, Some(revision));
                assert_eq!(result.conversion, Conversion::Text(expected.into()));
                if revision == 1 {
                    assert!(result.candidates.len() >= 3);
                    assert!(
                        result
                            .candidates
                            .iter()
                            .any(|candidate| candidate == "香水")
                    );
                }
                return true;
            }
            false
        });
    }

    let revision = 4;
    assert!(background.submit(RuntimeInput {
        revision,
        text: "a".repeat(4096),
        mode: InputMode::Compose,
    }));
    wait(
        &background,
        |event| matches!(event, Event::Started(key) if key.input_revision == Some(revision)),
    );
    background.invalidate(revision + 1);
    let deadline = Instant::now() + Duration::from_secs(60);
    while background.acknowledged() < revision + 1 {
        assert!(
            Instant::now() < deadline,
            "Host invalidation did not settle"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(background.submit(RuntimeInput {
        revision: 6,
        text: "kya".into(),
        mode: InputMode::Compose,
    }));
    wait(&background, |event| {
        if let Event::Converted(key, result) = event {
            assert_eq!(
                key.input_revision,
                Some(6),
                "invalidated request was delivered"
            );
            assert_eq!(result.conversion, Conversion::Text("きゃ".into()));
            return true;
        }
        false
    });
    drop(background);
    completion
        .recv_timeout(Duration::from_secs(15))
        .expect("Host and pipe threads did not close");
    println!(
        "Official IME Host compose, finalize, empty text, invalidation and process close passed."
    );
}
