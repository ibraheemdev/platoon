use crate::util::{poll_fn, LocalCell, UsizeHasher};

use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::task::{Context, Poll, Waker};

pub struct Semaphore {
    inner: LocalCell<Inner>,
}

pub struct Inner {
    permits: usize,
    waiters: HashMap<usize, Entry, BuildHasherDefault<UsizeHasher>>,
}

struct Entry {
    waker: Option<Waker>,
    required: usize,
    state: State,
}

#[derive(PartialEq)]
enum State {
    Waiting,
    Notified,
}

impl Semaphore {
    pub fn new(permits: usize) -> Self {
        Semaphore {
            inner: LocalCell::new(Inner {
                permits,
                waiters: HashMap::default(),
            }),
        }
    }

    pub async fn acquire(&self, permits: usize) -> Release<'_> {
        let mut entry = None;
        poll_fn(|cx| unsafe {
            self.inner
                .with(|i| i.poll_acquire(permits, &mut entry, cx))
                .map(|_| Release {
                    semaphore: self,
                    permits,
                })
        })
        .await
    }

    pub fn try_acquire(&self, permits: usize) -> Option<Release<'_>> {
        unsafe {
            self.inner.with(|i| {
                i.try_acquire(permits).then(|| Release {
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
                i.notify_last();
            })
        }
    }
}

pub struct Release<'a> {
    semaphore: &'a Semaphore,
    permits: usize,
}

impl Release<'_> {
    pub fn forget(mut self) {
        self.permits = 0;
    }
}

impl Drop for Release<'_> {
    fn drop(&mut self) {
        self.semaphore.add_permits(self.permits);
    }
}

impl Inner {
    fn poll_acquire(
        &mut self,
        permits: usize,
        entry: &mut Option<usize>,
        cx: &mut Context<'_>,
    ) -> Poll<()> {
        match entry {
            None => {
                if self.try_acquire(permits) {
                    Poll::Ready(())
                } else {
                    let next = self.waiters.len();
                    self.waiters.insert(
                        next,
                        Entry {
                            waker: Some(cx.waker().clone()),
                            state: State::Waiting,
                            required: permits,
                        },
                    );
                    *entry = Some(next);
                    Poll::Pending
                }
            }
            Some(i) => {
                let entry = self
                    .waiters
                    .get_mut(i)
                    .expect("future polled after completion");

                match entry.state {
                    State::Waiting => {
                        match &mut entry.waker {
                            Some(w) if w.will_wake(cx.waker()) => {}
                            _ => {
                                entry.waker = Some(cx.waker().clone());
                            }
                        }
                        Poll::Pending
                    }
                    State::Notified => {
                        assert!(self.permits >= entry.required);
                        self.permits -= entry.required;

                        self.waiters.remove(i).unwrap();
                        self.notify_last();

                        Poll::Ready(())
                    }
                }
            }
        }
    }

    fn notify_last(&mut self) {
        if let Some(entry) = self.waiters.get_mut(&(self.waiters.len() - 1)) {
            if self.permits < entry.required {
                return;
            }

            if entry.state != State::Notified {
                entry.state = State::Notified;

                if let Some(waker) = &entry.waker {
                    waker.wake_by_ref();
                }
            }
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
