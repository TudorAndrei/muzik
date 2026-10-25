use crate::{Error, Result};
use parking_lot::{Condvar, Mutex, MutexGuard};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::DerefMut;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;
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

    #[must_use]
    pub const fn limit(self) -> usize {
        match self {
            Self::Download => 2,
            Self::Process | Self::Import => 1,
        }
    }
}

struct Entry {
    ticket: u64,
    label: String,
}

struct Lane {
    active: Vec<Entry>,
    waiting: Vec<Entry>,
}

impl Lane {
    const fn new() -> Self {
        Self {
            active: Vec::new(),
            waiting: Vec::new(),
        }
    }
}

#[derive(Default)]
struct Context {
    label: String,
    held: Vec<(Gate, u64)>,
}

struct State<T> {
    download: Lane,
    process: Lane,
    import: Lane,
    next: u64,
    threads: HashMap<T, Context>,
}

impl<T: Eq + Hash> State<T> {
    fn new() -> Self {
        Self {
            download: Lane::new(),
            process: Lane::new(),
            import: Lane::new(),
            next: 0,
            threads: HashMap::new(),
        }
    }

    const fn lane(&self, gate: Gate) -> &Lane {
        match gate {
            Gate::Download => &self.download,
            Gate::Process => &self.process,
            Gate::Import => &self.import,
        }
    }

    const fn lane_mut(&mut self, gate: Gate) -> &mut Lane {
        match gate {
            Gate::Download => &mut self.download,
            Gate::Process => &mut self.process,
            Gate::Import => &mut self.import,
        }
    }

    fn context(&mut self, thread: T) -> &mut Context {
        self.threads.entry(thread).or_default()
    }

    fn held(&self, thread: &T) -> Vec<(Gate, u64)> {
        self.threads
            .get(thread)
            .map(|context| context.held.clone())
            .unwrap_or_default()
    }

    fn describe(&self) -> Value {
        let mut lanes = serde_json::Map::new();
        for gate in Gate::ALL.iter().copied() {
            let lane = self.lane(gate);
            let names = |entries: &[Entry]| {
                entries
                    .iter()
                    .map(|entry| entry.label.clone())
                    .collect::<Vec<_>>()
            };
            lanes.insert(
                gate.to_string(),
                json!({"limit":gate.limit(),"active":names(&lane.active),"waiting":names(&lane.waiting)}),
            );
        }
        Value::Object(lanes)
    }
}

trait Monitor: Send + Sync {
    type Thread: Copy + Eq + Hash + Send;
    type Guard<'a>: DerefMut<Target = State<Self::Thread>>
    where
        Self: 'a;

    fn current() -> Self::Thread;
    fn lock(&self) -> Self::Guard<'_>;
    fn wait<'a>(&'a self, guard: Self::Guard<'a>) -> Self::Guard<'a>;
    fn notify_all(&self);
}

type Listener = Box<dyn Fn(Value) + Send>;

struct Gates<M> {
    monitor: M,
    listener: Mutex<Option<Listener>>,
}

struct GatePermit<'a, M: Monitor> {
    gates: &'a Gates<M>,
    held: Option<(Gate, u64)>,
}

impl<M: Monitor> Drop for GatePermit<'_, M> {
    fn drop(&mut self) {
        if let Some((gate, ticket)) = self.held {
            self.gates.release(gate, ticket);
        }
    }
}

impl<M: Monitor> Gates<M> {
    fn new(monitor: M) -> Self {
        Self {
            monitor,
            listener: Mutex::new(None),
        }
    }

    fn listen(&self, listener: impl Fn(Value) + Send + 'static) {
        *self.listener.lock() = Some(Box::new(listener));
        self.publish(&self.monitor.lock());
    }

    fn set_label(&self, label: &str) {
        label.clone_into(&mut self.monitor.lock().context(M::current()).label);
    }

    fn enter(&self, gate: Gate, cancelled: &AtomicBool) -> Result<GatePermit<'_, M>> {
        let thread = M::current();
        let mut state = self.monitor.lock();
        let context = state.context(thread);
        if context.held.iter().any(|(held, _)| *held == gate) {
            return Ok(GatePermit {
                gates: self,
                held: None,
            });
        }
        let label = context.label.clone();
        let ticket = state.next;
        state.next = ticket.wrapping_add(1);
        state.lane_mut(gate).waiting.push(Entry { ticket, label });
        self.publish(&state);
        let mut state = self.admit(state, gate, ticket, cancelled)?;
        state.context(thread).held.push((gate, ticket));
        drop(state);
        Ok(GatePermit {
            gates: self,
            held: Some((gate, ticket)),
        })
    }

    fn release(&self, gate: Gate, ticket: u64) {
        let mut state = self.monitor.lock();
        state
            .context(M::current())
            .held
            .retain(|(_, held)| *held != ticket);
        state
            .lane_mut(gate)
            .active
            .retain(|entry| entry.ticket != ticket);
        self.monitor.notify_all();
        self.publish(&state);
    }

    fn suspended<T>(&self, work: impl FnOnce() -> T) -> T {
        let mut state = self.monitor.lock();
        let held = state.held(&M::current());
        if held.is_empty() {
            drop(state);
            return work();
        }
        let mut parked = Vec::new();
        for (gate, ticket) in held {
            let lane = state.lane_mut(gate);
            if let Some(index) = lane.active.iter().position(|entry| entry.ticket == ticket) {
                parked.push((gate, lane.active.remove(index)));
            }
        }
        self.monitor.notify_all();
        self.publish(&state);
        drop(state);
        let result = work();
        let never = AtomicBool::new(false);
        for (gate, entry) in parked {
            let ticket = entry.ticket;
            let mut state = self.monitor.lock();
            let waiting = &mut state.lane_mut(gate).waiting;
            let at = waiting.partition_point(|waiter| waiter.ticket < ticket);
            waiting.insert(at, entry);
            self.publish(&state);
            drop(self.admit(state, gate, ticket, &never));
        }
        result
    }

    fn admit<'a>(
        &'a self,
        mut state: M::Guard<'a>,
        gate: Gate,
        ticket: u64,
        cancelled: &AtomicBool,
    ) -> Result<M::Guard<'a>> {
        loop {
            let lane = state.lane_mut(gate);
            if cancelled.load(Ordering::SeqCst) {
                lane.waiting.retain(|entry| entry.ticket != ticket);
                self.monitor.notify_all();
                self.publish(&state);
                return Err(Error::GateCancelled(gate));
            }
            if lane.active.len() < gate.limit()
                && lane.waiting.first().map(|entry| entry.ticket) == Some(ticket)
            {
                let entry = lane.waiting.remove(0);
                lane.active.push(entry);
                self.monitor.notify_all();
                self.publish(&state);
                return Ok(state);
            }
            state = self.monitor.wait(state);
        }
    }

    fn snapshot(&self) -> Value {
        self.monitor.lock().describe()
    }

    fn publish(&self, state: &State<M::Thread>) {
        if let Some(listener) = self.listener.lock().as_ref() {
            listener(state.describe());
        }
    }
}

struct Parking {
    state: Mutex<State<ThreadId>>,
    changed: Condvar,
}

impl Monitor for Parking {
    type Thread = ThreadId;
    type Guard<'a> = MutexGuard<'a, State<ThreadId>>;

    fn current() -> ThreadId {
        std::thread::current().id()
    }

    fn lock(&self) -> Self::Guard<'_> {
        self.state.lock()
    }

    fn wait<'a>(&'a self, mut guard: Self::Guard<'a>) -> Self::Guard<'a> {
        self.changed
            .wait_for(&mut guard, Duration::from_millis(200));
        guard
    }

    fn notify_all(&self) {
        self.changed.notify_all();
    }
}

static GATES: LazyLock<Gates<Parking>> = LazyLock::new(|| {
    Gates::new(Parking {
        state: Mutex::new(State::new()),
        changed: Condvar::new(),
    })
});

pub struct Permit {
    _held: GatePermit<'static, Parking>,
}

pub fn listen(listener: impl Fn(Value) + Send + 'static) {
    GATES.listen(listener);
}

pub fn set_label(label: &str) {
    GATES.set_label(label);
}

/// # Errors
/// Returns an error when the wait for the gate is cancelled.
pub fn enter(gate: Gate, cancelled: &AtomicBool) -> Result<Permit> {
    Ok(Permit {
        _held: GATES.enter(gate, cancelled)?,
    })
}

pub fn suspended<T>(work: impl FnOnce() -> T) -> T {
    GATES.suspended(work)
}

pub fn snapshot() -> Value {
    GATES.snapshot()
}

#[cfg(test)]
mod tests {
    use super::{Gate, enter, set_label, snapshot, suspended};
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
    fn a_gate_admits_its_limit_and_the_next_waits() {
        let never = AtomicBool::new(false);
        set_label("gate holder");
        let first = enter(Gate::Import, &never).unwrap();
        let again = enter(Gate::Import, &never).unwrap();
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
        waiter.join().unwrap().unwrap();
        assert!(listed(Gate::Import, "active", "gate holder"));
        drop(again);
        drop(first);
        assert!(!listed(Gate::Import, "active", "gate holder"));
        let cancelled = AtomicBool::new(true);
        let blocked = thread::spawn(move || enter(Gate::Import, &cancelled).map(drop));
        assert!(blocked.join().unwrap().is_err());
    }
}
