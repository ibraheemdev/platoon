use crate::runtime::core::{Core, Direction, JoinHandle};

use std::cell::UnsafeCell;
use std::future::Future;
use std::io;
use std::os::unix::prelude::RawFd;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};
use std::time::Instant;

#[derive(Clone)]
pub struct Shared {
    pub core: Rc<UnsafeCell<Core>>,
}

impl Shared {
    unsafe fn into_core(&self) -> &mut Core {
        &mut *self.core.get()
    }

    pub fn insert_source(&self, raw: RawFd) -> io::Result<usize> {
        unsafe { self.into_core().insert_source(raw) }
    }

    pub fn remove_source(&self, key: usize) -> io::Result<()> {
        unsafe { self.into_core().remove_source(key) }
    }

    pub fn poll_ready(
        &self,
        key: usize,
        direction: Direction,
        cx: &mut Context<'_>,
    ) -> Poll<io::Result<()>> {
        unsafe { self.into_core().poll_ready(key, direction, cx) }
    }

    pub fn insert_alarm(&self, at: Instant, waker: Waker) -> usize {
        unsafe { self.into_core().insert_alarm(at, waker) }
    }

    pub fn remove_alarm(&self, id: usize, at: Instant) {
        unsafe { self.into_core().remove_alarm(id, at) }
    }

    pub fn reset_alarm(&self, id: usize, old: Instant, new: Instant) {
        unsafe { self.into_core().reset_alarm(id, old, new) }
    }

    pub fn replace_alarm_waker(&self, id: usize, deadline: Instant, waker: Waker) {
        unsafe { self.into_core().replace_alarm_waker(id, deadline, waker) }
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
    {
        unsafe { self.into_core().spawn(self.core.clone(), future) }
    }

    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        unsafe { self.into_core().block_on(self.core.clone(), future) }
    }
}
