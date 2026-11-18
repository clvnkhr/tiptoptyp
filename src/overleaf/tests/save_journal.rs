use super::*;

const ORIGINAL: [&str; 2] = ["α\r\nmain original", "章😀\nchapter original"];
const EDITED: [&str; 2] = ["α\r\nmain edited", "章😀\nchapter edited"];
const IDS: [&str; 2] = ["a", "b"];

#[derive(Clone, Copy, Debug)]
enum Action {
    Edit(usize, bool),
    Save(usize),
    Confirm,
    Reconnect,
    Hydrate,
    Discard(usize),
}
const ACTIONS: [Action; 11] = [
    Action::Edit(0, false),
    Action::Edit(0, true),
    Action::Edit(1, false),
    Action::Edit(1, true),
    Action::Save(0),
    Action::Save(1),
    Action::Confirm,
    Action::Reconnect,
    Action::Hydrate,
    Action::Discard(0),
    Action::Discard(1),
];

/// The oracle is a user-intent buffer and a durable server journal, not a
/// second implementation of Model's draft/conflict representation. Only an
/// explicit server receipt changes the journal; snapshots never confirm saves.
struct Journal {
    server: [String; 2],
    editor: [String; 2],
    pending: Option<(usize, BrowserCommand)>,
    retired: Vec<BrowserEvent>,
    epoch: u64,
    hydrated: bool,
    last_request: u64,
}
impl Journal {
    fn new() -> Self {
        Self {
            server: ORIGINAL.map(str::to_owned),
            editor: ORIGINAL.map(str::to_owned),
            pending: None,
            retired: Vec::new(),
            epoch: 7,
            hydrated: true,
            last_request: 0,
        }
    }

    fn step(&mut self, state: &mut Model, action: Action) {
        match action {
            Action::Edit(index, edited) => {
                let text = if edited {
                    EDITED[index]
                } else {
                    ORIGINAL[index]
                };
                let changed = self.editor[index] != text;
                assert_eq!(state.set_draft(IDS[index], text.into()).unwrap(), changed);
                self.editor[index] = text.into();
            }
            Action::Save(index) => {
                let command = state.publish(IDS[index]);
                if self.pending.is_some() || !self.hydrated {
                    assert!(command.is_none());
                } else {
                    let command = command.expect("explicit saves require a fresh receipt");
                    let BrowserCommand::Edit {
                        id,
                        base,
                        source,
                        request,
                        epoch,
                    } = &command
                    else {
                        panic!("unexpected save command: {command:?}");
                    };
                    assert_eq!(id, IDS[index]);
                    assert_eq!(base, &self.server[index]);
                    assert_eq!(source, &self.editor[index]);
                    assert_eq!(*epoch, self.epoch);
                    assert!(
                        *request > self.last_request,
                        "receipt identifiers must never be reused"
                    );
                    self.last_request = *request;
                    self.pending = Some((index, command));
                }
            }
            Action::Confirm => {
                if let Some((index, command)) = self.pending.take() {
                    let ack = acknowledgement(&command);
                    let BrowserEvent::Ack {
                        request, source, ..
                    } = &ack
                    else {
                        unreachable!()
                    };
                    self.server[index].clone_from(source);
                    assert_eq!(
                        state.apply(ack.clone()).unwrap(),
                        ModelChange::Acknowledged {
                            id: IDS[index].into(),
                            request: *request,
                            source: source.clone(),
                            dirty: self.editor[index] != self.server[index],
                        }
                    );
                    self.retired.push(ack);
                }
            }
            Action::Reconnect => {
                if let Some((_, command)) = self.pending.take() {
                    self.retired.push(acknowledgement(&command));
                }
                self.epoch += 1;
                self.hydrated = false;
                state.reconnect(self.epoch);
            }
            Action::Hydrate => {
                state.apply(project(self.epoch)).unwrap();
                for (index, source) in self.server.iter().enumerate() {
                    state
                        .apply(snapshot(self.epoch, IDS[index], source))
                        .unwrap();
                }
                self.hydrated = true;
            }
            Action::Discard(index) => {
                let allowed = !self
                    .pending
                    .as_ref()
                    .is_some_and(|(owner, _)| *owner == index);
                assert_eq!(state.release_native_draft(IDS[index]), allowed);
                if allowed {
                    self.editor[index].clone_from(&self.server[index]);
                }
            }
        }
        self.check(state);
    }

    fn check(&self, state: &mut Model) {
        let before = visible_state(state);
        if let Some((index, command)) = &self.pending {
            let BrowserCommand::Edit {
                id,
                base,
                source,
                request,
                epoch,
            } = command
            else {
                unreachable!()
            };
            // Browser store updates are optimistic; neither the old source nor
            // the submitted bytes is a durable save receipt.
            for text in [base, source] {
                assert_eq!(
                    state.apply(snapshot(*epoch, id, text)).unwrap(),
                    ModelChange::Ignored
                );
            }
            // Exact source bytes do not grant an unrelated receipt authority.
            for (epoch, owner, receipt) in [
                (*epoch - 1, IDS[*index], *request),
                (*epoch, IDS[1 - *index], *request),
                (*epoch, IDS[*index], u64::MAX),
            ] {
                for conflict in [false, true] {
                    let event = if conflict {
                        BrowserEvent::Conflict {
                            epoch,
                            id: owner.into(),
                            request: receipt,
                            source: source.clone(),
                            message: "stale".into(),
                        }
                    } else {
                        BrowserEvent::Ack {
                            epoch,
                            id: owner.into(),
                            request: receipt,
                            source: source.clone(),
                        }
                    };
                    assert_eq!(state.apply(event).unwrap(), ModelChange::Ignored);
                }
            }
        }
        for ack in self.retired.iter().rev().take(4) {
            assert_eq!(state.apply(ack.clone()).unwrap(), ModelChange::Ignored);
        }
        assert_eq!(
            visible_state(state),
            before,
            "unsolicited events changed state"
        );
        assert_eq!(state.epoch(), self.epoch);
        for (index, id) in IDS.iter().enumerate() {
            let document = state.document(id).unwrap();
            assert_eq!(document.baseline, self.server[index]);
            assert_eq!(document.source(), self.editor[index]);
            assert_eq!(
                document.is_dirty(),
                self.editor[index] != self.server[index]
            );
            assert!(
                document.conflict().is_none(),
                "a receipt-only history cannot conflict"
            );
        }
        assert_eq!(
            state.has_unsynced(),
            self.pending.is_some() || self.editor != self.server
        );
        let actual = state.in_flight().map(|pending| BrowserCommand::Edit {
            id: pending.id.clone(),
            base: pending.base.clone(),
            source: pending.source.clone(),
            request: pending.request,
            epoch: state.epoch(),
        });
        assert_eq!(
            actual.as_ref(),
            self.pending.as_ref().map(|(_, command)| command)
        );
    }
}

fn acknowledgement(command: &BrowserCommand) -> BrowserEvent {
    let BrowserCommand::Edit {
        id,
        source,
        request,
        epoch,
        ..
    } = command
    else {
        unreachable!()
    };
    BrowserEvent::Ack {
        epoch: *epoch,
        id: id.clone(),
        request: *request,
        source: source.clone(),
    }
}
fn visible_state(state: &Model) -> ([Option<model::Document>; 2], Option<model::InFlight>, bool) {
    (
        IDS.map(|id| state.document(id).cloned()),
        state.in_flight().cloned(),
        state.has_unsynced(),
    )
}
fn run(schedule: &[Action]) {
    let mut state = Model::new(7);
    state.apply(project(7)).unwrap();
    for (id, source) in IDS.into_iter().zip(ORIGINAL) {
        state.apply(snapshot(7, id, source)).unwrap();
    }
    let mut journal = Journal::new();
    for (step, action) in schedule.iter().copied().enumerate() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            journal.step(&mut state, action)
        }));
        if let Err(error) = result {
            eprintln!("save-journal counterexample: {:?}", &schedule[..=step]);
            std::panic::resume_unwind(error);
        }
    }
}

#[test]
fn save_journal_exhausts_short_two_document_schedules() {
    // 11^4 schedules include edit/save/edit/ack and save/reconnect/hydrate/save.
    // Every prefix is checked, so failures print a short reproducible history.
    for number in 0..ACTIONS.len().pow(4) {
        let mut number = number;
        let schedule = std::array::from_fn::<_, 4, _>(|_| {
            let action = ACTIONS[number % ACTIONS.len()];
            number /= ACTIONS.len();
            action
        });
        run(&schedule);
    }
    // Longer deterministic histories exercise request generations, repeated
    // reconnects and multiple parked edits without unbounded randomized runs.
    for seed in 1u64..=32 {
        let mut random = seed;
        let schedule = std::array::from_fn::<_, 64, _>(|_| {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            ACTIONS[random as usize % ACTIONS.len()]
        });
        run(&schedule);
    }
}
