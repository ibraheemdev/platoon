use crate::clock::Clock;
use crate::executor::{Executor, JoinHandle};
use crate::reactor::Reactor;
use crate::util::{self, RcWake};

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::thread::{self, ThreadId};
use std::time::Duration;

thread_local! {
    pub(crate) static RUNTIME: RefCell<Option<Runtime>> = RefCell::new(None);
}

#[derive(Clone)]
pub struct Runtime {
    shared: Rc<Shared>,
}

struct Shared {
    clock: Clock,
    reactor: Reactor,
    executor: Executor,
    woke_up: Cell<bool>,
    created_on: ThreadId,
}

const POLLS_PER_TICK: usize = 61;
const INITIAL_SOURCES: usize = 64;
const INITIAL_TASKS: usize = 64;

impl Runtime {
    pub fn new() -> io::Result<Self> {
        Reactor::with_capacity(INITIAL_SOURCES).map(|reactor| Self {
            shared: Rc::new(Shared {
                reactor,
                woke_up: Cell::new(false),
                clock: Clock::new(),
                executor: Executor::with_capacity(INITIAL_TASKS),
                created_on: thread::current().id(),
            }),
        })
    }

    pub fn set(&self) {
        RUNTIME.with(|rt| rt.borrow_mut().replace(self.clone()));
    }

    pub fn current() -> Option<Self> {
        RUNTIME.with(|rt| rt.borrow().clone())
    }

    pub fn block_on<F>(&self, mut future: F) -> F::Output
    where
        F: Future,
    {
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        let waker = self.shared.clone().into_waker();
        let mut cx = Context::from_waker(&waker);

        // make sure the main future is polled
        // on the first iteration of 'run
        self.shared.woke_up.set(true);

        'block_on: loop {
            if self.shared.woke_up.replace(false) {
                if let Poll::Ready(val) = future.as_mut().poll(&mut cx) {
                    return val;
                }
            }

            for _ in 0..POLLS_PER_TICK {
                if !self.shared.executor.poll_one() {
                    // there are no tasks to run, so
                    // just park until the next timer
                    // or IO event
                    self.drive_io(|next_timer| next_timer);
                    continue 'block_on;
                }

                // polling this task woke up the main
                // future
                if self.shared.woke_up.get() {
                    continue 'block_on;
                }
            }

            self.drive_io(|_| Some(Duration::from_millis(0)));
        }
    }

    fn drive_io(&self, duration: impl Fn(Option<Duration>) -> Option<Duration>) {
        let mut wakers = Vec::new();

        let next_timer = self.shared.clock.take_past_alarms(&mut wakers);

        self.shared
            .reactor
            .run(duration(next_timer), &mut wakers)
            .unwrap();

        self.shared.clock.take_past_alarms(&mut wakers);

        for waker in wakers {
            util::wake(waker);
        }
    }
}

unsafe impl RcWake for Shared {
    fn wake(self: Rc<Self>) {
        self.woke_up.set(true);
    }

    fn created_on(&self) -> ThreadId {
        self.created_on
    }
}

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
{
    RUNTIME.with(|rt| {
        rt.borrow()
            .as_ref()
            .expect(util::NO_RUNTIME)
            .shared
            .executor
            .spawn(future)
    })
}

pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    RUNTIME.with(|rt| {
        rt.borrow()
            .as_ref()
            .expect(util::NO_RUNTIME)
            .block_on(future)
    })
}
