use crate::runtime::Park;
use crate::{pin, util};

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};
use std::time::Duration;
use std::{io, mem};

pub struct Executor<P> {
    shared: Rc<Shared<P>>,
}

impl<P> Clone for Executor<P> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}

struct Shared<P> {
    queue: RefCell<VecDeque<Task>>,
    woke: Cell<bool>,
    created_on: Cell<ThreadId>,
    parker: P,
}

const POLLS_PER_TICK: usize = 61;
const INITIAL_SOURCES: usize = 64;
const INITIAL_TASKS: usize = 64;
const INITIAL_EVENTS: usize = 1024;

impl<P> Executor<P>
where
    P: Park,
{
    pub fn new(park: P) -> io::Result<Self> {
        Ok(Executor {
            shared: Rc::new(Shared {
                queue: RefCell::new(VecDeque::with_capacity(INITIAL_TASKS)),
                woke: Cell::new(false),
                created_on: Cell::new(thread::current().id()),
                parker: park,
            }),
        })
    }

    pub fn spawn<F>(&self, future: F) -> Task
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        let task = Task {
            raw: Rc::new(TaskRepr {
                executor: self.clone(),
                awaiter: Cell::new(None),
                state: Cell::new(State::Scheduled),
                payload: RefCell::new(Payload::Future(future)),
            }),
        };

        self.shared.queue.borrow_mut().push_back(task.clone());

        task
    }

    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        let waker = self.shared.clone().into_waker();

        pin!(future);
        let mut cx = Context::from_waker(&waker);

        // make sure the main future is polled
        // on the first iteration of 'run
        self.shared.woke.set(true);

        'block_on: loop {
            if self.shared.woke.replace(false) {
                if let Poll::Ready(val) = future.as_mut().poll(&mut cx) {
                    return val;
                }
            }

            for _ in 0..POLLS_PER_TICK {
                let task = self.shared.queue.borrow_mut().pop_front();

                match task {
                    Some(task) => task.raw.run(),
                    None => {
                        // there are no tasks to run, so
                        // just park until the next timer
                        // or IO event
                        let mut wakers = Vec::new();
                        self.shared
                            .parker
                            .park(&mut wakers)
                            .expect("failed to park thread");
                        wakers.into_iter().for_each(util::wake);
                        continue 'block_on;
                    }
                }

                // polling this task woke up the main
                // future
                if self.shared.woke.get() {
                    continue 'block_on;
                }
            }

            let mut wakers = Vec::new();
            self.shared
                .parker
                .park_timeout(Duration::ZERO, &mut wakers)
                .expect("failed to park thread");
            wakers.into_iter().for_each(util::wake);
        }
    }
}

unsafe impl<P> RcWake for Shared<P> {
    fn wake(self: Rc<Self>) {
        self.woke.set(true)
    }

    fn created_on(&self) -> ThreadId {
        self.created_on.get()
    }
}

trait RawTask {
    fn run(self: Rc<Self>);
    fn poll_output(&self, cx: &mut Context<'_>, out: &mut dyn Any);
    fn cancel(&self, out: &mut dyn Any);
}

#[derive(Clone)]
pub struct Task {
    raw: Rc<dyn RawTask>,
}

impl Task {
    pub fn poll<T: 'static>(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        let mut out = Poll::Pending;
        self.raw.poll_output(cx, &mut out as _);
        out
    }

    pub fn cancel<T: 'static>(self) -> Option<T> {
        let mut out = None;
        self.raw.cancel(&mut out as _);
        out
    }
}

struct TaskRepr<F: Future, P> {
    executor: Executor<P>,
    awaiter: Cell<Option<Waker>>,
    state: Cell<State>,
    payload: RefCell<Payload<F>>,
}

enum Payload<F: Future> {
    Future(F),
    Output(F::Output),
    Taken,
}

impl<F: Future> Payload<F> {
    fn take(&mut self) -> Self {
        mem::replace(self, Payload::Taken)
    }
}

#[derive(Copy, Clone)]
enum State {
    Scheduled,
    Idle,
    Done,
}

impl<F, P> RawTask for TaskRepr<F, P>
where
    F: Future + 'static,
    P: 'static,
{
    fn poll_output(&self, cx: &mut Context<'_>, out: &mut dyn Any) {
        let mut payload = self.payload.borrow_mut();
        match *payload {
            Payload::Output(_) => match payload.take() {
                Payload::Output(val) => {
                    *out.downcast_mut::<Poll<F::Output>>().unwrap() = Poll::Ready(val)
                }
                _ => unreachable!(),
            },
            Payload::Taken => panic!("JoinHandle polled after completion"),
            _ => self.awaiter.set(Some(cx.waker().clone())),
        }
    }

    fn run(self: Rc<Self>) {
        let waker = self.clone().into_waker();
        let mut cx = Context::from_waker(&waker);

        self.state.set(State::Idle);

        let done = {
            let mut payload = self.payload.borrow_mut();
            match *payload {
                Payload::Future(ref mut future) => {
                    match unsafe { Pin::new_unchecked(future).poll(&mut cx) } {
                        Poll::Ready(val) => {
                            *payload = Payload::Output(val);
                            true
                        }
                        Poll::Pending => false,
                    }
                }
                _ => false,
            }
        };

        if done {
            self.state.set(State::Done);
            if let Some(waker) = self.awaiter.take() {
                util::wake(waker);
            }
        }
    }

    fn cancel(&self, out: &mut dyn Any) {
        if let Payload::Output(val) = self.payload.borrow_mut().take() {
            *out.downcast_mut::<Option<F::Output>>().unwrap() = Some(val);
        }
    }
}

unsafe impl<F, P> RcWake for TaskRepr<F, P>
where
    F: Future + 'static,
    P: 'static,
{
    fn wake(self: Rc<Self>) {
        match self.state.get() {
            State::Idle => {
                self.state.set(State::Scheduled);
                self.executor
                    .shared
                    .queue
                    .borrow_mut()
                    .push_back(Task { raw: self.clone() });
            }
            State::Scheduled | State::Done => {}
        }
    }

    fn created_on(&self) -> ThreadId {
        self.executor.shared.created_on()
    }
}

/// # Safety
///
/// `created_on` must return the id of thread that the waker was created on.
unsafe trait RcWake {
    fn wake(self: Rc<Self>);
    fn created_on(&self) -> ThreadId;

    fn into_waker(self: Rc<Self>) -> Waker
    where
        Self: Sized + 'static,
    {
        fn assert_not_sent(waker: &Rc<impl RcWake>) {
            if thread::current().id() != waker.created_on() {
                panic!("cannot use waker from outside the thread it was created on");
            }
        }

        unsafe fn clone<W: RcWake>(waker: *const ()) -> RawWaker {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            mem::forget(waker.clone());

            RawWaker::new(
                Rc::into_raw(waker) as *const (),
                &RawWakerVTable::new(clone::<W>, wake::<W>, wake_by_ref::<W>, drop::<W>),
            )
        }

        unsafe fn wake<W: RcWake>(waker: *const ()) {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            W::wake(waker);
        }

        unsafe fn wake_by_ref<W: RcWake>(waker: *const ()) {
            let waker = unsafe { ManuallyDrop::new(Rc::from_raw(waker as *const W)) };
            assert_not_sent(&waker);
            W::wake((*waker).clone());
        }

        unsafe fn drop<W: RcWake>(waker: *const ()) {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            let _ = waker;
        }

        let raw = RawWaker::new(
            Rc::into_raw(self) as *const (),
            &RawWakerVTable::new(
                clone::<Self>,
                wake::<Self>,
                wake_by_ref::<Self>,
                drop::<Self>,
            ),
        );

        unsafe { Waker::from_raw(raw) }
    }
}
