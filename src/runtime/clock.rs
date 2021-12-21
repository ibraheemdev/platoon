use crate::runtime::Park;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::task::Waker;
use std::time::{Duration, Instant};
use std::{io, mem};

#[derive(Clone)]
pub struct Clock<P> {
    shared: Rc<RefCell<Shared<P>>>,
}

struct Shared<P> {
    parker: P,
    next_alarm: usize,
    alarms: BTreeMap<(Instant, usize), Waker>,
}

impl<P> Clock<P> {
    pub fn new(parker: P) -> io::Result<Self> {
        Ok(Clock {
            shared: Rc::new(RefCell::new(Shared {
                next_alarm: 0,
                alarms: BTreeMap::new(),
                parker,
            })),
        })
    }

    pub fn insert_alarm(&self, at: Instant, waker: Waker) -> usize {
        let mut shared = self.shared.borrow_mut();
        let id = shared.next_alarm;
        shared.alarms.insert((at, id), waker);
        shared.next_alarm += 1;
        id
    }

    pub fn remove_alarm(&self, id: usize, at: Instant) {
        self.shared.borrow_mut().alarms.remove(&(at, id));
    }

    pub fn reset_alarm(&self, id: usize, old: Instant, new: Instant) {
        let mut shared = self.shared.borrow_mut();
        let waker = shared.alarms.remove(&(old, id)).unwrap();
        shared.alarms.insert((new, id), waker);
    }

    pub fn replace_alarm_waker(&self, id: usize, deadline: Instant, waker: Waker) {
        *self
            .shared
            .borrow_mut()
            .alarms
            .get_mut(&(deadline, id))
            .unwrap() = waker;
    }
}

impl<P> Shared<P> {
    fn extend_with_expired(&mut self, wakers: &mut Vec<Waker>) -> Option<Duration> {
        let now = Instant::now();

        let later = self.alarms.split_off(&(now + Duration::from_nanos(1), 0));
        let ready = mem::replace(&mut self.alarms, later);

        match ready.len() {
            0 => self
                .alarms
                .keys()
                .next()
                .map(|(when, _)| when.saturating_duration_since(now))
                // round up to 1ms
                .map(|d| Duration::from_millis(d.as_millis() as u64)),
            _ => {
                wakers.extend(ready.into_values());
                Some(Duration::from_secs(0))
            }
        }
    }
}

impl<P> Park for Clock<P>
where
    P: Park,
{
    fn park(&self, wakers: &mut Vec<Waker>) -> io::Result<()> {
        let mut shared = self.shared.borrow_mut();
        let next = shared.extend_with_expired(wakers);
        match next {
            Some(next) => shared.parker.park_timeout(next, wakers),
            None => shared.parker.park(wakers),
        }
    }

    fn park_timeout(&self, mut duration: Duration, wakers: &mut Vec<Waker>) -> io::Result<()> {
        let mut shared = self.shared.borrow_mut();
        if let Some(next) = shared.extend_with_expired(wakers) {
            duration = duration.min(next);
        }

        shared.parker.park_timeout(duration, wakers)
    }
}
