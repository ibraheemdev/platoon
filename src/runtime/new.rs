use std::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, RawWaker, Waker},
    thread::ThreadId,
};

use futures::Future;

use crate::util::LocalCell;

use super::core::{Core, RcWake};

pub(crate) struct Task {
    raw: Rc<LocalCell<dyn RawTask>>,
}

impl Task {
    pub fn assume_init(self) -> Rc<T> {
        self.raw.assume_init()
    }
}

// fn new_task(core: Core, future: impl Future<Output = *mut ()> + 'static) -> Task {
//     Rc::new(LocalCell::new(RawTask {
//         core,
//         value: None,
//         waiter: None,
//         state: State::Idle,
//         stage: Stage
//     }))
// }

pub(crate) struct RawTask<F: Future> {
    core: Core,
    state: State,
    stage: Stage<F>,
}

enum Stage<F: Future> {
    Future(F),
    Ready(F::Output),
}

unsafe impl<F: Future> RcWake for LocalCell<RawTask<F>> {
    fn wake(self: Rc<Self>) {
        unsafe {
            self.with(|task| match task.state {
                State::Idle => {
                    task.state = State::Scheduled;
                    unsafe {
                        task.core.shared.with(|shared| {
                            shared.new_queue.push_back(self.clone());
                        })
                    }
                }
                _ => {}
            })
        }
    }

    fn created_on(&self) -> ThreadId {
        unsafe { self.with(|task| task.core.shared.with(|shared| shared.created_on)) }
    }
}

pub struct JoinHandle<T> {
    task: Task,
    _t: PhantomData<T>,
}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            self.task.with(|task| match task.state {
                State::Complete => unsafe {
                    let val = Box::from_raw(task.value.unwrap() as *mut T);
                    Poll::Ready(*val)
                },
                _ => {
                    task.waiter = Some(cx.waker().clone());
                    Poll::Pending
                }
            })
        }
    }
}

#[derive(Clone, Copy)]
enum State {
    Idle,
    Scheduled,
    Complete,
}
