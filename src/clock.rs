use std::cell::{Cell, UnsafeCell};
use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use crate::{util, Runtime};

pub struct Clock {
    alarms: UnsafeCell<BTreeMap<(Instant, usize), Waker>>,
    next_id: Cell<usize>,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            alarms: UnsafeCell::new(BTreeMap::new()),
            next_id: Cell::new(0),
        }
    }

    pub fn insert_alarm(&self, at: Instant, waker: Waker) -> usize {
        let id = self.next_id.get();
        unsafe {
            (*self.alarms.get()).insert((at, id), waker);
        }
        self.next_id.set(id + 1);
        id
    }

    pub fn remove_alarm(&self, id: usize, at: Instant) {
        unsafe {
            (*self.alarms.get()).remove(&(at, id));
        }
    }

    pub fn reset_alarm(&self, id: usize, old: Instant, new: Instant) {
        let alarms = unsafe { &mut *self.alarms.get() };
        let waker = alarms.remove(&(old, id)).unwrap();
        alarms.insert((new, id), waker);
    }

    pub fn replace_alarm_waker(&self, id: usize, deadline: Instant, waker: Waker) {
        unsafe {
            *(*self.alarms.get()).get_mut(&(deadline, id)).unwrap() = waker;
        }
    }

    pub fn take_past_alarms(&self, wakers: &mut Vec<Waker>) -> Option<Duration> {
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

        // Round up to 1ms here to avoid very short durations
        // that might be treated as zero-length.
        // TODO: this cast could be bad
        dur.map(|d| Duration::from_millis(d.as_millis() as _))
    }
}

pub fn sleep(duration: Duration) -> Sleep {
    sleep_until(Instant::now() + duration)
}

pub fn sleep_until(deadline: Instant) -> Sleep {
    Sleep {
        deadline,
        alarm: None,
        runtime: Runtime::current().expect(util::NO_RUNTIME),
    }
}

pub struct Sleep {
    deadline: Instant,
    alarm: Option<(usize, Waker)>,
    runtime: Runtime,
}

impl Sleep {
    pub fn reset(&mut self, deadline: Instant) {
        if let Some((id, _)) = self.alarm.as_mut() {
            self.runtime
                .shared
                .clock
                .reset_alarm(*id, self.deadline, deadline);
        }

        self.deadline = deadline;
    }
}

impl Future for Sleep {
    type Output = Instant;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let Sleep {
            deadline,
            alarm,
            runtime,
        } = &mut *self;

        if Instant::now() >= *deadline {
            return Poll::Ready(*deadline);
        }

        match alarm {
            Some((id, waker)) if !waker.will_wake(cx.waker()) => {
                runtime
                    .shared
                    .clock
                    .replace_alarm_waker(*id, *deadline, cx.waker().clone());
                *waker = cx.waker().clone();
            }
            Some(_) => {}
            None => {
                let id = runtime
                    .shared
                    .clock
                    .insert_alarm(*deadline, cx.waker().clone());
                *alarm = Some((id, cx.waker().clone()));
            }
        }

        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some((id, _)) = self.alarm {
            self.runtime.shared.clock.remove_alarm(id, self.deadline);
        }
    }
}

pub fn interval(period: Duration) -> Interval {
    interval_at(Instant::now() + period, period)
}

pub fn interval_at(period: Instant, interval: Duration) -> Interval {
    Interval {
        interval,
        sleep: sleep_until(period),
    }
}

pub struct Interval {
    sleep: Sleep,
    interval: Duration,
}

impl Interval {
    pub async fn tick(&mut self) -> Option<Instant> {
        util::poll_fn(move |cx| {
            if Pin::new(&mut self.sleep).poll(cx).is_pending() {
                return Poll::Pending;
            }

            self.sleep.reset(Instant::now() + self.interval);

            Poll::Ready(Some(self.sleep.deadline))
        })
        .await
    }
}

pub fn timeout<T>(duration: Duration, future: T) -> Timeout<T>
where
    T: Future,
{
    timeout_at(Instant::now() + duration, future)
}

pub fn timeout_at<T>(deadline: Instant, future: T) -> Timeout<T>
where
    T: Future,
{
    Timeout {
        future,
        sleep: sleep_until(deadline),
    }
}

pin_project_lite::pin_project! {
    pub struct Timeout<F> {
        #[pin]
        future: F,
        sleep: Sleep
    }
}

impl<F> Future for Timeout<F>
where
    F: Future,
{
    type Output = Result<F::Output, TimedOut>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut this = self.project();

        if let Poll::Ready(v) = this.future.poll(cx) {
            return Poll::Ready(Ok(v));
        }

        match Pin::new(&mut this.sleep).poll(cx) {
            Poll::Ready(_) => Poll::Ready(Err(TimedOut { _priv: () })),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(PartialEq, Eq)]
pub struct TimedOut {
    _priv: (),
}

impl fmt::Debug for TimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TimedOut").finish()
    }
}

impl fmt::Display for TimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "future timed out")
    }
}

impl std::error::Error for TimedOut {}
