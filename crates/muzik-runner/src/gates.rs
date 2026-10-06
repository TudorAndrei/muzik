use crate::{Error, Result};
use parking_lot::{Condvar, Mutex, MutexGuard};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use strum_macros::{AsRefStr, Display, EnumString, IntoStaticStr, VariantArray};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, AsRefStr, Display, EnumString, IntoStaticStr, VariantArray,
)]
#[strum(serialize_all = "snake_case")]
pub enum Gate {
    Download,
    Process,
    Import,
}

impl Gate {
    pub const ALL: &'static [Self] = <Self as strum::VariantArray>::VARIANTS;

    fn index(self) -> usize {
        self as usize
    }

    pub fn limit(self) -> usize {
        match self {
            Self::Download => 2,
            Self::Process | Self::Import => 1,
        }
    }
}

#[derive(Default)]
struct Lane {
    active: Vec<(u64, String)>,
    waiting: Vec<(u64, String)>,
}

#[derive(Default)]
struct State {
    lanes: [Lane; 3],
    next: u64,
}

type Listener = Box<dyn Fn(Value) + Send>;

static STATE: Mutex<State> = Mutex::new(State {
    lanes: [
        Lane {
            active: Vec::new(),
            waiting: Vec::new(),
        },
        Lane {
            active: Vec::new(),
            waiting: Vec::new(),
        },
        Lane {
            active: Vec::new(),
            waiting: Vec::new(),
        },
    ],
    next: 0,
});
static CHANGED: Condvar = Condvar::new();
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

thread_local! {
    static LABEL: RefCell<String> = const { RefCell::new(String::new()) };
    static HELD: RefCell<Vec<(Gate, u64)>> = const { RefCell::new(Vec::new()) };
}

pub struct Permit {
    held: Option<(Gate, u64)>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let Some((gate, ticket)) = self.held else {
            return;
        };
        HELD.with(|held| held.borrow_mut().retain(|entry| entry.1 != ticket));
        let mut state = STATE.lock();
        state.lanes[gate.index()]
            .active
            .retain(|(active, _)| *active != ticket);
        CHANGED.notify_all();
        publish(&state);
    }
}

pub fn listen(listener: impl Fn(Value) + Send + 'static) {
    *LISTENER.lock() = Some(Box::new(listener));
    publish(&STATE.lock());
}

pub fn set_label(label: &str) {
    LABEL.with(|current| label.clone_into(&mut current.borrow_mut()));
}

pub fn enter(gate: Gate, cancelled: &AtomicBool) -> Result<Permit> {
    if HELD.with(|held| held.borrow().iter().any(|entry| entry.0 == gate)) {
        return Ok(Permit { held: None });
    }
    let label = LABEL.with(|label| label.borrow().clone());
    let mut state = STATE.lock();
    let ticket = state.next;
    state.next += 1;
    state.lanes[gate.index()].waiting.push((ticket, label));
    publish(&state);
    let state = admit(state, gate, ticket, cancelled)?;
    drop(state);
    HELD.with(|held| held.borrow_mut().push((gate, ticket)));
    Ok(Permit {
        held: Some((gate, ticket)),
    })
}

pub fn suspended<T>(work: impl FnOnce() -> T) -> T {
    let held = HELD.with(|held| held.borrow().clone());
    if held.is_empty() {
        return work();
    }
    let mut labels = Vec::new();
    {
        let mut state = STATE.lock();
        for (gate, ticket) in &held {
            let lane = &mut state.lanes[gate.index()];
            if let Some(index) = lane.active.iter().position(|entry| entry.0 == *ticket) {
                labels.push(lane.active.remove(index).1);
            }
        }
        CHANGED.notify_all();
        publish(&state);
    }
    let result = work();
    let never = AtomicBool::new(false);
    for ((gate, ticket), label) in held.into_iter().zip(labels) {
        let mut state = STATE.lock();
        let waiting = &mut state.lanes[gate.index()].waiting;
        let at = waiting.partition_point(|entry| entry.0 < ticket);
        waiting.insert(at, (ticket, label));
        publish(&state);
        if let Ok(state) = admit(state, gate, ticket, &never) {
            drop(state);
        }
    }
    result
}

fn admit(
    mut state: MutexGuard<'static, State>,
    gate: Gate,
    ticket: u64,
    cancelled: &AtomicBool,
) -> Result<MutexGuard<'static, State>> {
    loop {
        let lane = &mut state.lanes[gate.index()];
        if cancelled.load(Ordering::SeqCst) {
            lane.waiting.retain(|entry| entry.0 != ticket);
            CHANGED.notify_all();
            publish(&state);
            return Err(Error::GateCancelled(gate));
        }
        if lane.active.len() < gate.limit()
            && lane.waiting.first().map(|entry| entry.0) == Some(ticket)
        {
            let entry = lane.waiting.remove(0);
            lane.active.push(entry);
            CHANGED.notify_all();
            publish(&state);
            return Ok(state);
        }
        CHANGED.wait_for(&mut state, Duration::from_millis(200));
    }
}

pub fn snapshot() -> Value {
    describe(&STATE.lock())
}

fn describe(state: &State) -> Value {
    let mut lanes = serde_json::Map::new();
    for gate in Gate::ALL.iter().copied() {
        let lane = &state.lanes[gate.index()];
        let names = |entries: &[(u64, String)]| {
            entries
                .iter()
                .map(|entry| entry.1.clone())
                .collect::<Vec<_>>()
        };
        lanes.insert(
            gate.to_string(),
            json!({"limit":gate.limit(),"active":names(&lane.active),"waiting":names(&lane.waiting)}),
        );
    }
    Value::Object(lanes)
}

fn publish(state: &State) {
    if let Some(listener) = LISTENER.lock().as_ref() {
        listener(describe(state));
    }
}

#[cfg(test)]
mod tests {
    use super::{enter, set_label, snapshot, suspended, Gate};
    use serde_json::json;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    fn listed(lane: Gate, part: &str, label: &str) -> bool {
        snapshot()[lane.as_ref()][part]
            .as_array()
            .is_some_and(|names| names.contains(&json!(label)))
    }

    #[test]
    fn a_gate_admits_its_limit_and_the_next_waits() -> Result<(), String> {
        let never = AtomicBool::new(false);
        set_label("gate holder");
        let first = enter(Gate::Import, &never)?;
        let again = enter(Gate::Import, &never)?;
        let (sender, receiver) = mpsc::channel();
        let waiter = thread::spawn(move || {
            set_label("gate waiter");
            let never = AtomicBool::new(false);
            let permit = enter(Gate::Import, &never);
            let _ = sender.send(());
            permit.map(drop)
        });
        assert!(receiver.recv_timeout(Duration::from_millis(300)).is_err());
        assert!(listed(Gate::Import, "waiting", "gate waiter"));
        let during = suspended(|| receiver.recv_timeout(Duration::from_secs(5)));
        assert!(during.is_ok());
        waiter.join().map_err(|_| "waiter panicked")??;
        assert!(listed(Gate::Import, "active", "gate holder"));
        drop(again);
        drop(first);
        assert!(!listed(Gate::Import, "active", "gate holder"));
        let cancelled = AtomicBool::new(true);
        let blocked = thread::spawn(move || enter(Gate::Import, &cancelled).map(drop));
        assert!(blocked.join().map_err(|_| "blocked panicked")?.is_err());
        Ok(())
    }
}
