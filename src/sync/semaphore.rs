use crate::util::LocalCell;

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use slab::Slab;

pub struct Semaphore {
    inner: LocalCell<Inner>,
}

pub struct Inner {
    permits: usize,
    last_entry: usize,
    entries: Slab<Entry>,
}

struct Entry {
    waker: Option<Waker>,
    required: usize,
    notified: bool,
}

enum AcquireState {
    Idle,
    Waiting(usize),
    Done,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Semaphore {
            inner: LocalCell::new(Inner {
                permits,
                last_entry: 0,
                entries: Slab::new(),
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
                unsafe {
                    self.semaphore
                        .inner
                        .with(|i| i.poll_acquire(self.permits, &mut self.state, cx))
                        .map(|_| Permit {
                            semaphore: self.semaphore,
                            permits: self.permits,
                        })
                }
            }
        }

        impl Drop for Acquire<'_> {
            fn drop(&mut self) {
                if let AcquireState::Waiting(entry) = self.state {
                    unsafe {
                        self.semaphore.inner.with(|i| {
                            i.entries.remove(entry);
                            i.notify();
                        })
                    }
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
        unsafe {
            self.inner.with(|i| {
                i.try_acquire(permits).then(|| Permit {
                    semaphore: self,
                    permits,
                })
            })
        }
    }

    pub fn permits(&self) -> usize {
        unsafe { self.inner.with(|i| i.permits) }
    }

    pub fn add_permits(&self, permits: usize) {
        if permits == 0 {
            return;
        }

        unsafe {
            self.inner.with(|i| {
                i.permits += permits;
                i.notify();
            })
        }
    }
}

pub struct Permit<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
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
    fn poll_acquire(
        &mut self,
        permits: usize,
        state: &mut AcquireState,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        match state {
            AcquireState::Idle => {
                if self.try_acquire(permits) {
                    Poll::Ready(())
                } else {
                    let key = self.entries.insert(Entry {
                        waker: Some(cx.waker().clone()),
                        required: permits,
                        notified: false,
                    });
                    self.last_entry = key;
                    *state = AcquireState::Waiting(key);
                    Poll::Pending
                }
            }
            AcquireState::Waiting(i) => {
                let entry = self.entries.get_mut(*i).unwrap();

                if entry.notified {
                    assert!(self.permits >= entry.required);
                    self.permits -= entry.required;

                    self.entries.remove(*i);
                    self.notify();

                    *state = AcquireState::Done;
                    Poll::Ready(())
                } else {
                    match &mut entry.waker {
                        Some(w) if w.will_wake(cx.waker()) => {}
                        _ => {
                            entry.waker = Some(cx.waker().clone());
                        }
                    }
                    Poll::Pending
                }
            }
            AcquireState::Done => panic!("future polled after completion"),
        }
    }

    // TODO: potentially notify more than one waiter here
    fn notify(&mut self) {
        if let Some(entry) = self.entries.get_mut(self.last_entry) {
            if self.permits < entry.required {
                return;
            }

            if !entry.notified {
                entry.notified = true;

                if let Some(waker) = &entry.waker {
                    waker.wake_by_ref();
                }
            }
        }
    }

    fn try_acquire(&mut self, required: usize) -> bool {
        if (self.permits >= required) && (self.entries.is_empty() || required == 0) {
            self.permits -= required;
            true
        } else {
            false
        }
    }
}
