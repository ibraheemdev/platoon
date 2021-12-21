use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use slab::Slab;

pub struct Semaphore {
    inner: RefCell<Inner>,
}

pub struct Inner {
    permits: usize,
    waiters: Slab<Waiter>,
    head: Option<usize>,
    tail: Option<usize>,
}

#[derive(Default)]
struct Waiter {
    waker: Option<Waker>,
    next: Option<usize>,
    prev: Option<usize>,
    required: usize,
    woke: bool,
}

enum AcquireState {
    Idle,
    Waiting(usize),
    Done,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Semaphore {
            inner: RefCell::new(Inner {
                permits,
                head: None,
                tail: None,
                waiters: Slab::new(),
            }),
        }
    }

    pub async fn acquire(&self, permits: usize) -> Permit<'_> {
        struct Acquire<'a> {
            semaphore: &'a Semaphore,
            state: AcquireState,
            permits: usize,
        }

        impl<'a> Future for Acquire<'a> {
            type Output = Permit<'a>;

            fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                self.semaphore
                    .inner
                    .borrow_mut()
                    .poll_acquire(self.permits, &mut self.state, cx)
                    .map(|_| Permit {
                        semaphore: self.semaphore,
                        permits: self.permits,
                    })
            }
        }

        impl Drop for Acquire<'_> {
            fn drop(&mut self) {
                if let AcquireState::Waiting(i) = self.state {
                    let mut inner = self.semaphore.inner.borrow_mut();
                    inner.remove(i);
                    inner.wake();
                }
            }
        }

        Acquire {
            semaphore: self,
            state: AcquireState::Idle,
            permits,
        }
        .await
    }

    pub fn try_acquire(&self, permits: usize) -> Option<Permit<'_>> {
        let mut inner = self.inner.borrow_mut();
        inner.try_acquire(permits).then(|| Permit {
            semaphore: self,
            permits,
        })
    }

    pub fn permits(&self) -> usize {
        self.inner.borrow().permits
    }

    pub fn add_permits(&self, permits: usize) {
        if permits == 0 {
            return;
        }

        let mut inner = self.inner.borrow_mut();
        inner.permits += permits;
        inner.wake();
    }
}

pub struct Permit<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
}

// TODO
impl std::fmt::Debug for Permit<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "")
    }
}

impl Permit<'_> {
    pub fn forget(mut self) {
        self.permits = 0;
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.semaphore.add_permits(self.permits);
    }
}

impl Inner {
    fn add(&mut self, waker: Waker, required: usize) -> usize {
        let key = self.waiters.insert(Waiter {
            waker: Some(waker),
            required,
            woke: false,
            next: self.head,
            prev: None,
        });

        if let Some(head) = self.head.replace(key) {
            self.waiters[head].prev = Some(key)
        }

        if self.tail.is_none() {
            self.tail = Some(key);
        }

        key
    }

    fn remove(&mut self, key: usize) {
        let waiter = self.waiters.remove(key);

        match waiter.prev {
            None => self.head = waiter.next,
            Some(prev) => self.waiters[prev].next = waiter.next,
        }

        match waiter.next {
            None => self.tail = waiter.prev,
            Some(next) => self.waiters[next].prev = waiter.prev,
        }
    }

    fn poll_acquire(
        &mut self,
        permits: usize,
        state: &mut AcquireState,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        match *state {
            AcquireState::Idle => {
                if self.try_acquire(permits) {
                    Poll::Ready(())
                } else {
                    let key = self.add(cx.waker().clone(), permits);
                    *state = AcquireState::Waiting(key);
                    Poll::Pending
                }
            }
            AcquireState::Waiting(i) => {
                let waiter = &mut self.waiters[i];

                if waiter.woke {
                    assert!(self.permits >= waiter.required);
                    self.permits -= waiter.required;

                    self.remove(i);
                    self.wake();

                    *state = AcquireState::Done;
                    Poll::Ready(())
                } else {
                    match &mut waiter.waker {
                        Some(w) if w.will_wake(cx.waker()) => {}
                        _ => {
                            waiter.waker = Some(cx.waker().clone());
                        }
                    }
                    Poll::Pending
                }
            }
            AcquireState::Done => panic!("future polled after completion"),
        }
    }

    fn wake(&mut self) {
        let mut permits = self.permits;
        let mut tail = self.tail;

        while let Some(waiter) = tail {
            let waiter = &mut self.waiters[waiter];

            if permits < waiter.required {
                break;
            }

            permits -= waiter.required;

            if !waiter.woke {
                waiter.woke = true;

                if let Some(waker) = &waiter.waker {
                    waker.wake_by_ref();
                }
            }

            tail = waiter.prev;
        }
    }

    fn try_acquire(&mut self, required: usize) -> bool {
        if (self.permits >= required) && (self.waiters.is_empty() || required == 0) {
            self.permits -= required;
            true
        } else {
            false
        }
    }
}
