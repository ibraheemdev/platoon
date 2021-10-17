use std::cell::{Cell, UnsafeCell};
use std::collections::VecDeque;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::thread::{self, ThreadId};

use crate::util::RcWake;

pub struct Executor {
    queue: TaskQueue,
}

type TaskQueue = Rc<UnsafeCell<VecDeque<Task>>>;

impl Executor {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            queue: Rc::new(UnsafeCell::new(VecDeque::with_capacity(capacity))),
        }
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
    {
        let task = Rc::new(InnerTask {
            value: Cell::new(None),
            waiter: Cell::new(None),
            queue: self.queue.clone(),
            state: Cell::new(State::Waiting),
            created_on: thread::current().id(),
            future: UnsafeCell::new(Box::pin(async move {
                Box::into_raw(Box::new(future.await)) as *mut ()
            })),
        });

        unsafe {
            (*self.queue.get()).push_back(task.clone());
        }

        JoinHandle {
            task,
            _t: PhantomData,
        }
    }

    pub fn poll_one(&self) -> bool {
        unsafe {
            if let Some(task) = (*self.queue.get()).pop_front() {
                task.state.set(State::Polling);
                let waker = task.clone().into_waker();
                let mut cx = Context::from_waker(&waker);
                let mut fut = Pin::new_unchecked(&mut *task.future.get());

                if let Poll::Ready(val) = fut.as_mut().poll(&mut cx) {
                    task.value.set(Some(val));
                    task.state.set(State::Complete);

                    if let Some(waker) = task.waiter.take() {
                        waker.wake();
                    }
                } else {
                    task.state.set(State::Waiting);
                }

                return true;
            }
        }

        false
    }
}

pub struct JoinHandle<T> {
    task: Task,
    _t: PhantomData<T>,
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match self.task.state.get() {
            State::Complete => unsafe {
                let val = Box::from_raw(self.task.value.get().unwrap() as *mut T);
                Poll::Ready(*val)
            },
            _ => {
                self.task.waiter.set(Some(cx.waker().clone()));
                Poll::Pending
            }
        }
    }
}

type Task = Rc<InnerTask>;

pub(crate) struct InnerTask {
    queue: TaskQueue,
    state: Cell<State>,
    value: Cell<Option<*mut ()>>,
    waiter: Cell<Option<Waker>>,
    future: UnsafeCell<Pin<Box<dyn Future<Output = *mut ()>>>>,
    created_on: ThreadId,
}

unsafe impl RcWake for InnerTask {
    fn wake(self: Rc<Self>) {
        loop {
            match self.state.get() {
                State::Waiting => {
                    self.state.set(State::Polling);
                    unsafe {
                        (*self.queue.get()).push_back(self.clone());
                    }
                    break;
                }
                State::Polling => {
                    self.state.set(State::Repoll);
                    break;
                }
                _ => break,
            }
        }
    }

    fn created_on(&self) -> ThreadId {
        self.created_on
    }
}

#[derive(Clone, Copy)]
enum State {
    Waiting,
    Polling,
    Repoll,
    Complete,
}
