use crate::clock::Clock;
use crate::executor::Executor;
use crate::reactor::Reactor;
use crate::util;

use std::cell::RefCell;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};
use std::time::Duration;

thread_local! {
    pub(crate) static RUNTIME: RefCell<Option<Runtime>> = RefCell::new(None);
}

#[derive(Clone)]
pub struct Runtime {
    inner: Rc<Inner>,
}

struct Inner {
    clock: Clock,
    reactor: Reactor,
    executor: Executor,
}

const POLLS_PER_TICK: usize = 61;
const INITIAL_SOURCES: usize = 64;
const INITIAL_TASKS: usize = 64;

impl Runtime {
    pub fn new() -> io::Result<Self> {
        Reactor::with_capacity(INITIAL_SOURCES).map(|reactor| Self {
            inner: Rc::new(Inner {
                reactor,
                clock: Clock::new(),
                executor: Executor::with_capacity(INITIAL_TASKS),
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
        let waker = util::noop_waker();
        let mut cx = Context::from_waker(&waker);

        loop {
            if let Poll::Ready(val) = future.as_mut().poll(&mut cx) {
                return val;
            }

            for _ in 0..POLLS_PER_TICK {
                if !self.inner.executor.poll_one() {
                    // there are no tasks to run, so
                    // just park until the next timer
                    // or IO event
                    self.drive_io(|next_timer| next_timer);
                }
            }

            self.drive_io(|_| Some(Duration::from_millis(0)));
        }
    }

    fn drive_io(&self, duration: impl Fn(Option<Duration>) -> Option<Duration>) {
        let mut wakers = Vec::new();

        let next_timer = self.inner.clock.take_ready(&mut wakers);

        self.inner
            .reactor
            .run(duration(next_timer), &mut wakers)
            .unwrap();

        self.inner.clock.take_ready(&mut wakers);

        for waker in wakers {
            util::wake(waker);
        }
    }
}

pub fn spawn<F>(future: F)
where
    F: Future<Output = ()> + 'static,
{
    RUNTIME.with(|rt| {
        rt.borrow()
            .as_ref()
            .expect(util::NO_RUNTIME)
            .inner
            .executor
            .spawn(future);
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
