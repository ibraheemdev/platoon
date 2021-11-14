//! Asynchronous timers.
//!
//! The timers in this module operate at millisecond granularity.
//!
//! # Examples
//!
//! ```no_run
//! use std::time::Duration;
//!
//! fn main() {
//!     platoon::block_on(async {
//!         platoon::sleep(Duration::from_secs(1)).await;
//!         println!("Hello after 1 second");
//!     });
//! }
//! ```
use crate::{util, Runtime};

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

/// Puts the current task to sleep for the specified amount of time.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// # platoon::block_on(async {
/// platoon::sleep(Duration::from_secs(1)).await;
/// println!("Hello after 1 second");
/// # });
/// ```
pub fn sleep(duration: Duration) -> Sleep {
    sleep_until(Instant::now() + duration)
}

/// Puts the current task to sleep until `deadline`.
///
/// # Examples
///
/// ```no_run
/// use std::time::{Duration, Instant};
///
/// # platoon::block_on(async {
/// platoon::sleep_until(Instant::now() + Duration::from_secs(1)).await;
/// println!("Hello after 1 second");
/// # });
/// ```
pub fn sleep_until(deadline: Instant) -> Sleep {
    Sleep {
        deadline,
        alarm: None,
        runtime: Runtime::unwrap_current(),
    }
}

/// Future returned by [`sleep`] and [`sleep_until`].
pub struct Sleep {
    deadline: Instant,
    alarm: Option<(usize, Waker)>,
    runtime: Runtime,
}

impl Sleep {
    /// Returns the instant at which the future will complete.
    pub fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Resets the timer to a new deadline.
    pub fn reset(&mut self, deadline: Instant) {
        if let Some((id, _)) = self.alarm.as_mut() {
            self.runtime.core.reset_alarm(*id, self.deadline, deadline);
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
                    .core
                    .replace_alarm_waker(*id, *deadline, cx.waker().clone());
                *waker = cx.waker().clone();
            }
            Some(_) => {}
            None => {
                let id = runtime.core.insert_alarm(*deadline, cx.waker().clone());
                *alarm = Some((id, cx.waker().clone()));
            }
        }

        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some((id, _)) = self.alarm {
            self.runtime.core.remove_alarm(id, self.deadline);
        }
    }
}

/// Returns an `Interval` that ticks every specified duration.
///
/// THe first tick returns immediately. Use [`interval_at`] if you
/// want to customize the start time.
///
/// # Examples
///
/// ```no_run
/// use std::time::{Duration};
/// use platoon::time::interval;
///
/// # platoon::block_on(async {
/// let mut interval = interval(Duration::from_secs(1));
/// for i in 0..5 {
///     interval.tick().await;
///     println!("tick");
/// }
/// # });
/// ```
pub fn interval(interval: Duration) -> Interval {
    interval_at(Instant::now(), interval)
}

/// Returns an `Interval` that ticks every specified duration.
///
/// The first tick returns at `start`.
///
/// ```no_run
/// use std::time::{Instant, Duration};
/// use platoon::time::interval_at;
///
/// # platoon::block_on(async {
/// let mut interval = interval_at(Instant::now() + Duration_from_secs(5), Duration::from_secs(1));
///
/// interval.tick().await; // 5 secs
/// interval.tick().await; // 6 secs
/// interval.tick().await; // 7 secs
///
/// println!("It's been 5 seconds");
/// # });
/// ```
pub fn interval_at(start: Instant, interval: Duration) -> Interval {
    Interval {
        interval,
        sleep: sleep_until(start),
    }
}

/// An interval timer.
///
/// See [`interval`] and [`interval_at`] for details.
pub struct Interval {
    sleep: Sleep,
    interval: Duration,
}

impl Interval {
    /// Completes when the next interval is reached.
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

/// Causes a future to time out after a given duration of time.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
/// use platoon::time::timeout;
///
/// # async fn fetch_record() {}
/// # platoon::block_on(async {
/// match timeout(Duration_from_millis(5), fetch_user()) {
///     Ok(user) => {
///         // ...
///     },
///     Err(_) => {
///         println!("took over 5 ms to fetch user")
///     }
/// }
/// });
/// ```
pub fn timeout<T>(duration: Duration, future: T) -> Timeout<T>
where
    T: Future,
{
    timeout_at(Instant::now() + duration, future)
}

/// Causes a future to time out at the given deadline.
///
/// # Examples
///
/// ```
/// use std::time::{Instant, Duration};
/// use platoon::time::timeout;
///
/// # async fn fetch_record() {}
/// # platoon::block_on(async {
/// match timeout_at(Instant::now() + Duration_from_millis(5), fetch_user()) {
///     Ok(user) => {
///         // ...
///     },
///     Err(_) => {
///         println!("took over 5 ms to fetch user")
///     }
/// }
/// });
/// ```
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
    /// Future returned by [`timeout`] and [`timeout_at`].
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

/// Error returned by [`Timeout`].
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
