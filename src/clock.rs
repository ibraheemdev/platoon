use std::cell::{Cell, UnsafeCell};
use std::collections::BTreeMap;
use std::mem;
use std::task::Waker;
use std::time::{Duration, Instant};

pub struct Clock {
    alarms: UnsafeCell<BTreeMap<(Instant, usize), Waker>>,
    next_id: Cell<usize>,
}

unsafe impl Send for Clock {}

impl Clock {
    pub fn new() -> Self {
        Self {
            alarms: UnsafeCell::new(BTreeMap::new()),
            next_id: Cell::new(0),
        }
    }

    pub fn insert(&self, at: Instant, waker: &Waker) -> usize {
        let id = self.next_id.get();
        unsafe {
            (*self.alarms.get()).insert((at, id), waker.clone());
        }
        self.next_id.set(id + 1);
        id
    }

    pub fn remove(&self, at: Instant, id: usize) {
        unsafe {
            (*self.alarms.get()).remove(&(at, id)).unwrap();
        }
    }

    pub fn take_ready(&self, wakers: &mut Vec<Waker>) -> Option<Duration> {
        let map = unsafe { &mut *self.alarms.get() };
        let now = Instant::now();

        let later = map.split_off(&(now, 0));
        let ready = mem::replace(map, later);

        let dur = if ready.is_empty() {
            map.keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
        } else {
            Some(Duration::from_secs(0))
        };

        for waker in ready.into_values() {
            wakers.push(waker);
        }

        // Round up to 1ms here - this avoids
        // very short-duration microsecond-resolution sleeps that the OS
        // might treat as zero-length.
        // TODO: this cast could be bad
        dur.map(|d| Duration::from_millis(d.as_millis() as _))
    }
}
