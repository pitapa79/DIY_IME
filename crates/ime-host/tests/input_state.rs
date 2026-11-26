use diy_ime2_ime_host::{
    Application, CompositionId, Conversion, Delivery, DocumentStamp, Edit, EditOutcome,
    EditSession, EditSnapshot, EditTarget, InputScope, InputState, Plan, RequestId, StateError,
};
use diy_ime2_runtime::{
    execution_control::{DecimalId, RunKey, SessionEpoch},
    node::InputMode,
};

fn id(value: u64) -> DecimalId {
    DecimalId::new(value).unwrap()
}

fn plan(version: u64) -> Plan {
    Plan {
        session: SessionEpoch::new("cc00cc00cc00cc00cc00cc00cc00cc00").unwrap(),
        version: id(version),
    }
}

struct FakeDocument {
    stamp: DocumentStamp,
    text: String,
    preview: Option<(CompositionId, String)>,
    commits: usize,
    fail: Option<Failure>,
    selection: EditSnapshot,
}

enum Failure {
    BeforeWrite,
    AfterWrite,
}

impl FakeDocument {
    fn new() -> Self {
        Self {
            stamp: DocumentStamp {
                focus: 1,
                context: 1,
                revision: 1,
                selection: 1,
            },
            text: "本文".into(),
            preview: None,
            commits: 0,
            fail: None,
            selection: EditSnapshot {
                utf16: "𠮷".encode_utf16().collect(),
                range: 2..2,
            },
        }
    }
}

impl EditSession for FakeDocument {
    fn snapshot(&mut self, target: &EditTarget) -> Option<EditSnapshot> {
        if target.document != self.stamp
            || self.preview.as_ref().map(|(owner, _)| owner)
                != target.has_composition_range.then_some(&target.composition)
        {
            return None;
        }
        Some(EditSnapshot {
            utf16: self.selection.utf16.clone(),
            range: self.selection.range.clone(),
        })
    }

    fn edit(&mut self, target: &EditTarget, edit: Edit<'_>) -> EditOutcome {
        assert_eq!(target.document, self.stamp);
        let fail = self.fail.take();
        if matches!(fail, Some(Failure::BeforeWrite)) {
            return EditOutcome::Unchanged;
        }
        match edit {
            Edit::Preview(text) => self.preview = Some((target.composition.clone(), text.into())),
            Edit::Commit(text) => {
                if !text.is_empty() {
                    self.commits += 1;
                    self.text.push_str(text);
                }
                self.preview = None;
            }
        }
        self.stamp.revision += 1;
        if matches!(fail, Some(Failure::AfterWrite)) {
            EditOutcome::Uncertain
        } else {
            EditOutcome::Applied {
                document: self.stamp,
                has_composition_range: self.preview.is_some(),
            }
        }
    }
}

fn setup() -> (InputState, FakeDocument) {
    let document = FakeDocument::new();
    let mut state = InputState::new(plan(1));
    assert!(!state.accepts_input());
    let observation = state
        .observe_document(Some(document.stamp), InputScope::Editable)
        .unwrap();
    assert!(!state.accepts_input());
    assert!(state.acknowledge_invalidation(&observation));
    (state, document)
}

fn submit(state: &mut InputState, request: &RequestId, mode: InputMode, text: &str) -> RunKey {
    state
        .dispatch(request, |plan, input| {
            assert_eq!((mode, text), (input.mode, input.text.as_str()));
            RunKey {
                session_epoch: plan.session.clone(),
                plan_version: plan.version,
                input_revision: Some(input.revision),
                run_id: id(input.revision + 30),
                delivery_generation: id(input.revision + 50),
            }
        })
        .unwrap()
}

fn deliver(state: &mut InputState, request: &RequestId, key: RunKey, text: &str) {
    state.accept_run(request, key.clone()).unwrap();
    assert_eq!(
        state.receive(request, &key, Conversion::Text(text.into())),
        Ok(Delivery::Ready)
    );
}

#[test]
fn commit_retry_requires_a_definite_no_write_and_consumes_the_result_once() {
    let (mut state, mut document) = setup();
    let compose = state.replace_input("n".into()).unwrap();
    let compose_key = submit(&mut state, &compose, InputMode::Compose, "n");
    deliver(&mut state, &compose, compose_key, "n");
    assert_eq!(document.text, "本文");
    assert!(matches!(
        state.apply(&compose, &mut document),
        Ok(Application::Applied)
    ));
    assert_eq!(document.preview.as_ref().unwrap().1, "n");

    let commit = state.commit().unwrap();
    assert_eq!(state.commit(), Err(StateError::Busy));
    let finalize_key = submit(&mut state, &commit, InputMode::Finalize, "n");
    deliver(&mut state, &commit, finalize_key.clone(), "ん");
    assert_eq!(document.text, "本文");

    document.fail = Some(Failure::BeforeWrite);
    assert!(matches!(
        state.apply(&commit, &mut document),
        Ok(Application::Unchanged)
    ));
    assert_eq!(document.text, "本文");
    assert!(matches!(
        state.apply(&commit, &mut document),
        Ok(Application::Applied)
    ));
    assert!(matches!(
        state.apply(&commit, &mut document),
        Err(StateError::Stale)
    ));
    assert_eq!(
        state.receive(&commit, &finalize_key, Conversion::Text("ん".into())),
        Err(StateError::Stale)
    );
    assert_eq!(state.commit(), Err(StateError::NoComposition));
    assert_eq!(document.text, "本文ん");
    assert_eq!(document.commits, 1);
}

#[test]
fn unknown_write_quarantines_the_snapshot_and_never_requests_speculative_cleanup() {
    let (mut state, mut document) = setup();
    state.replace_input("a".into()).unwrap();
    let commit = state.commit().unwrap();
    let key = submit(&mut state, &commit, InputMode::Finalize, "a");
    deliver(&mut state, &commit, key, "あ");
    let original = document.stamp;
    document.fail = Some(Failure::AfterWrite);

    let Application::Uncertain(invalidation) = state.apply(&commit, &mut document).unwrap() else {
        panic!("write occurred, completion was lost");
    };
    assert!(invalidation.cleanup.is_none());
    assert!(state.acknowledge_invalidation(&invalidation));
    assert!(
        state
            .observe_document(Some(original), InputScope::Editable)
            .is_none()
    );
    assert_eq!(state.replace_input("a".into()), Err(StateError::Protected));
    let observed = state
        .observe_document(Some(document.stamp), InputScope::Editable)
        .unwrap();
    assert!(state.acknowledge_invalidation(&observed));
    assert_eq!(state.commit(), Err(StateError::NoComposition));
    assert!(matches!(
        state.apply(&commit, &mut document),
        Err(StateError::Stale)
    ));
    assert_eq!(document.commits, 1);
}

#[test]
fn superseding_input_and_changed_focus_drop_queued_running_and_ready_results() {
    let (mut state, mut document) = setup();
    let old = state.replace_input("a".into()).unwrap();
    let old_key = submit(&mut state, &old, InputMode::Compose, "a");
    state.accept_run(&old, old_key.clone()).unwrap();
    let newer = state.replace_input("a".into()).unwrap();
    assert_eq!(
        state.receive(&old, &old_key, Conversion::Text("あ".into())),
        Err(StateError::Stale)
    );
    let newer_key = submit(&mut state, &newer, InputMode::Compose, "a");
    assert_ne!(newer_key.input_revision, old_key.input_revision);
    deliver(&mut state, &newer, newer_key, "あ");

    let lost = state.observe_document(None, InputScope::Unknown).unwrap();
    document.stamp.focus += 1;
    let gained = state
        .observe_document(Some(document.stamp), InputScope::Editable)
        .unwrap();
    assert!(!state.acknowledge_invalidation(&lost));
    assert_eq!(state.replace_input("a".into()), Err(StateError::Protected));
    assert!(state.acknowledge_invalidation(&gained));
    assert!(matches!(
        state.apply(&newer, &mut document),
        Err(StateError::Stale)
    ));
    assert_eq!(document.text, "本文");
    let newest = state.replace_input("a".into()).unwrap();
    let invalidation = state.cancel().unwrap();
    assert_eq!(
        state.dispatch(&newest, |_, _| panic!("canceled input dispatched")),
        Err::<(), _>(StateError::Stale)
    );
    assert!(state.acknowledge_invalidation(&invalidation));
}

#[test]
fn mismatched_run_keys_and_duplicates_leave_the_current_result_available() {
    let (mut state, mut document) = setup();
    let request = state.replace_input("ka".into()).unwrap();
    let key = submit(&mut state, &request, InputMode::Compose, "ka");
    let mut mismatch = key.clone();
    mismatch.session_epoch = SessionEpoch::new("00cc00cc00cc00cc00cc00cc00cc00cc").unwrap();
    assert_eq!(state.accept_run(&request, mismatch), Err(StateError::Stale));
    state.accept_run(&request, key.clone()).unwrap();

    let mut mismatch = key.clone();
    mismatch.run_id = id(658);
    assert_eq!(
        state.receive(&request, &mismatch, Conversion::Invalid),
        Err(StateError::Stale)
    );
    mismatch = key.clone();
    mismatch.delivery_generation = id(665);
    assert_eq!(
        state.receive(&request, &mismatch, Conversion::NoOutput),
        Err(StateError::Stale)
    );
    assert_eq!(
        state.receive(&request, &key, Conversion::Text("か".into())),
        Ok(Delivery::Ready)
    );
    assert_eq!(
        state.receive(&request, &key, Conversion::Text("旧".into())),
        Err(StateError::Stale)
    );
    assert!(matches!(
        state.apply(&request, &mut document),
        Ok(Application::Applied)
    ));
    assert_eq!(document.preview.unwrap().1, "か");
}

#[test]
fn protected_or_unknown_scope_and_unacknowledged_revocation_never_dispatch() {
    let (mut state, mut document) = setup();
    let request = state.replace_input("n".into()).unwrap();
    let key = submit(&mut state, &request, InputMode::Compose, "n");
    deliver(&mut state, &request, key, "n");
    let manual = state.set_manual_protection(true).unwrap();
    assert!(state.acknowledge_invalidation(&manual));
    assert!(matches!(
        state.apply(&request, &mut document),
        Err(StateError::Stale)
    ));

    for scope in [InputScope::Protected, InputScope::Unknown] {
        let observation = state.observe_document(Some(document.stamp), scope).unwrap();
        assert!(state.acknowledge_invalidation(&observation));
        assert_eq!(
            state.replace_input("fake secret".into()),
            Err(StateError::Protected)
        );
    }
    let release = state.set_manual_protection(false).unwrap();
    assert!(state.acknowledge_invalidation(&release));
    assert_eq!(
        state.replace_input("fake secret".into()),
        Err(StateError::Protected)
    );

    let restore = state
        .observe_document(Some(document.stamp), InputScope::Editable)
        .unwrap();
    assert!(!state.acknowledge_invalidation(&release));
    assert_eq!(
        state.replace_input("fake secret".into()),
        Err(StateError::Protected)
    );
    assert!(state.acknowledge_invalidation(&restore));
    assert!(state.replace_input("new".into()).is_ok());
}

#[test]
fn plan_switch_is_deferred_and_external_termination_drops_only_its_own_composition() {
    let (mut state, mut document) = setup();
    let request = state.replace_input("a".into()).unwrap();
    let key = submit(&mut state, &request, InputMode::Compose, "a");
    deliver(&mut state, &request, key, "あ");
    assert!(matches!(
        state.apply(&request, &mut document),
        Ok(Application::Applied)
    ));
    let owner = document.preview.as_ref().unwrap().0.clone();
    assert!(matches!(state.switch_plan(plan(2)), Err(StateError::Busy)));

    let cleanup = state.cancel().unwrap();
    assert_eq!(cleanup.cleanup.as_ref().unwrap().composition, owner);
    document.preview = None;
    assert!(state.acknowledge_invalidation(&cleanup));
    let change = state.switch_plan(plan(2)).unwrap().unwrap();
    assert!(state.acknowledge_invalidation(&change));
    let next = state.replace_input("a".into()).unwrap();
    let key = submit(&mut state, &next, InputMode::Compose, "a");
    assert_eq!(key.plan_version, id(2));
    assert!(state.composition_ended(&owner).is_none());
    deliver(&mut state, &next, key, "あ");
    assert!(matches!(
        state.apply(&next, &mut document),
        Ok(Application::Applied)
    ));
    let live_owner = &document.preview.as_ref().unwrap().0;
    let end = state.composition_ended(live_owner).unwrap();
    assert!(end.cleanup.is_none());
    assert_eq!(document.text, "本文");
}

#[test]
fn invalid_ranges_or_foreign_ownership_are_rejected_before_the_edit() {
    let (mut state, mut document) = setup();
    for (range, utf16) in [
        (1..1, vec![0xd842, 0xdfb7]),
        (0..1, vec![0xd842]),
        (std::ops::Range { start: 2, end: 1 }, vec![0x304b, 0x3099]),
        (0..3, vec![0x304b, 0x3099]),
    ] {
        document.selection = EditSnapshot { utf16, range };
        let request = state.replace_input("a".into()).unwrap();
        let key = submit(&mut state, &request, InputMode::Compose, "a");
        deliver(&mut state, &request, key, "あ");
        assert!(matches!(
            state.apply(&request, &mut document),
            Err(StateError::InvalidTarget)
        ));
        assert!(document.preview.is_none());
    }
    let request = state.replace_input("a".into()).unwrap();
    let key = submit(&mut state, &request, InputMode::Compose, "a");
    deliver(&mut state, &request, key, "あ");
    document.stamp.selection += 1;
    assert!(matches!(
        state.apply(&request, &mut document),
        Err(StateError::InvalidTarget)
    ));
    assert_eq!(document.text, "本文");
}

#[test]
fn no_output_and_invalid_output_do_not_apply_but_empty_text_is_a_valid_result() {
    let (mut state, mut document) = setup();
    state.replace_input(String::new()).unwrap();
    for (conversion, outcome) in [
        (Conversion::NoOutput, Delivery::NoOutput),
        (Conversion::Invalid, Delivery::Invalid),
    ] {
        let request = state.commit().unwrap();
        let key = submit(&mut state, &request, InputMode::Finalize, "");
        state.accept_run(&request, key.clone()).unwrap();
        assert_eq!(state.receive(&request, &key, conversion), Ok(outcome));
        assert!(matches!(
            state.apply(&request, &mut document),
            Err(StateError::Stale)
        ));
    }
    let request = state.commit().unwrap();
    let key = submit(&mut state, &request, InputMode::Finalize, "");
    deliver(&mut state, &request, key, "");
    assert!(matches!(
        state.apply(&request, &mut document),
        Ok(Application::Applied)
    ));
    assert_eq!(document.text, "本文");
    assert_eq!(document.commits, 0);
}
