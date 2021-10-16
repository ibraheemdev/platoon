use std::cell::{Cell, UnsafeCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};

pub struct Executor {
    queue: TaskQueue,
}

unsafe impl Send for Executor {}

type TaskQueue = Rc<UnsafeCell<VecDeque<Task>>>;

impl Executor {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            queue: Rc::new(UnsafeCell::new(VecDeque::with_capacity(capacity))),
        }
    }

    pub fn spawn<F>(&self, future: F)
    where
        F: Future<Output = ()> + 'static,
    {
        let task = Rc::new(InnerTask {
            thread_id: thread::current().id(),
            queue: self.queue.clone(),
            state: Cell::new(State::Waiting),
            future: UnsafeCell::new(Box::pin(future)),
        });

        unsafe {
            (*self.queue.get()).push_back(task);
        }
    }

    pub fn poll_one(&self) -> bool {
        unsafe {
            if let Some(task) = (*self.queue.get()).pop_front() {
                task.state.set(State::Polling);
                let waker = raw::waker(task.clone());
                let mut cx = Context::from_waker(&waker);
                let mut fut = Pin::new_unchecked(&mut *task.future.get());

                loop {
                    if fut.as_mut().poll(&mut cx).is_ready() {
                        break task.state.set(State::Complete);
                    }

                    task.state.set(State::Waiting);
                }

                return true;
            }
        }

        false
    }
}

type Task = Rc<InnerTask>;

pub(crate) struct InnerTask {
    thread_id: ThreadId,
    queue: TaskQueue,
    state: Cell<State>,
    future: UnsafeCell<Pin<Box<dyn Future<Output = ()>>>>,
}

impl InnerTask {
    fn assert_not_sent(&self) {
        if thread::current().id() != self.thread_id {
            panic!("cannot use waker from outside the thread it was created on");
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Waiting,
    Polling,
    Repoll,
    Complete,
}

mod raw {
    use super::*;

    use std::mem::{self, ManuallyDrop};

    pub(crate) unsafe fn waker(task: Rc<InnerTask>) -> Waker {
        Waker::from_raw(RawWaker::new(
            Rc::into_raw(task) as *const (),
            &RawWakerVTable::new(clone, wake, wake_ref, drop),
        ))
    }

    unsafe fn clone(task: *const ()) -> RawWaker {
        let task = Rc::from_raw(task as *const InnerTask);
        task.assert_not_sent();
        mem::forget(task.clone());

        RawWaker::new(
            Rc::into_raw(task) as _,
            &RawWakerVTable::new(clone, wake, wake_ref, drop),
        )
    }

    unsafe fn drop(task: *const ()) {
        let task = Rc::from_raw(task as *const InnerTask);
        task.assert_not_sent();
        mem::drop(task);
    }

    unsafe fn wake(task: *const ()) {
        let task = Rc::from_raw(task as *const InnerTask);
        task.assert_not_sent();

        loop {
            match task.state.get() {
                State::Waiting => {
                    task.state.set(State::Polling);
                    (*task.queue.get()).push_back(task.clone());
                    break;
                }
                State::Polling => {
                    task.state.set(State::Repoll);
                    break;
                }
                _ => break,
            }
        }
    }

    unsafe fn wake_ref(task: *const ()) {
        let task = ManuallyDrop::new(Rc::from_raw(task as *const InnerTask));
        task.assert_not_sent();

        loop {
            match task.state.get() {
                State::Waiting => {
                    task.state.set(State::Polling);
                    (*task.queue.get()).push_back((*task).clone());
                    break;
                }
                State::Polling => {
                    task.state.set(State::Repoll);
                    break;
                }
                _ => break,
            }
        }
    }
}
