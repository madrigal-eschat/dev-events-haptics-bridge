use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::gestures::Event;

struct State {
    queue: VecDeque<Event>,
    idle_magnitude: f32,
    // last-emitted magnitude per device; used by clear() to know what to turn off
    device_magnitudes: HashMap<u8, f32>,
    // set by interrupt/clear to abort the current event's sleep early
    woken: bool,
    shutdown: bool,
}

pub struct Player {
    state: Arc<Mutex<State>>,
    condvar: Arc<Condvar>,
    thread: Option<JoinHandle<()>>,
}

impl Player {
    pub fn new(callback: impl Fn(Event) + Send + 'static) -> Self {
        let state = Arc::new(Mutex::new(State {
            queue: VecDeque::new(),
            idle_magnitude: 0.0,
            device_magnitudes: HashMap::new(),
            woken: false,
            shutdown: false,
        }));
        let condvar = Arc::new(Condvar::new());

        let thread = {
            let state = Arc::clone(&state);
            let condvar = Arc::clone(&condvar);
            thread::spawn(move || player_thread(state, condvar, callback))
        };

        Self {
            state,
            condvar,
            thread: Some(thread),
        }
    }

    /// Append events to the back of the queue.
    pub fn queue(&self, events: &[Event]) {
        let mut s = self.state.lock().unwrap();
        s.queue.extend(events.iter().copied());
        self.condvar.notify_one();
    }

    /// Prepend events to the front of the queue and abort the current event's
    /// remaining duration so the new first event fires immediately.
    pub fn interrupt(&self, events: &[Event]) {
        let mut s = self.state.lock().unwrap();
        for &event in events.iter().rev() {
            s.queue.push_front(event);
        }
        s.woken = true;
        self.condvar.notify_one();
    }

    /// Clear the queue and immediately emit a zero-magnitude event for every
    /// device currently above zero.
    pub fn clear(&self) {
        let mut s = self.state.lock().unwrap();
        s.queue.clear();
        let off: Vec<Event> = s
            .device_magnitudes
            .iter()
            .filter(|&(_, &mag)| mag > 0.0)
            .map(|(&device, _)| Event::new(0, 0.0, device))
            .collect();
        for event in off.into_iter().rev() {
            s.queue.push_front(event);
        }
        s.woken = true;
        self.condvar.notify_one();
    }

    pub fn idle_magnitude(&self) -> f32 {
        self.state.lock().unwrap().idle_magnitude
    }

    pub fn set_idle_magnitude(&self, magnitude: f32) {
        self.state.lock().unwrap().idle_magnitude = magnitude;
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        {
            let mut s = self.state.lock().unwrap();
            s.shutdown = true;
            s.woken = true;
        }
        self.condvar.notify_one();
        if let Some(t) = self.thread.take() {
            t.join().ok();
        }
    }
}

fn player_thread(state: Arc<Mutex<State>>, condvar: Arc<Condvar>, callback: impl Fn(Event)) {
    loop {
        // Block until there is an event to emit. Tracking device magnitude
        // happens here under the lock so clear() always sees up-to-date state.
        let event = {
            let mut s = state.lock().unwrap();
            loop {
                if s.shutdown {
                    return;
                }
                if let Some(e) = s.queue.pop_front() {
                    s.device_magnitudes.insert(e.device, e.magnitude);
                    break e;
                }
                s = condvar.wait(s).unwrap();
            }
            // MutexGuard dropped here; callback runs without the lock.
        };

        callback(event);

        // Hold the lock for the event's duration. interrupt() / clear() set
        // woken=true and signal the condvar to cut the sleep short.
        if event.duration_ms > 0 {
            let mut s = state.lock().unwrap();
            if !s.woken {
                let (guard, _) = condvar
                    .wait_timeout(s, Duration::from_millis(event.duration_ms as u64))
                    .unwrap();
                s = guard;
            }
            s.woken = false;
            // MutexGuard dropped; loop immediately to next event.
        }
    }
}
