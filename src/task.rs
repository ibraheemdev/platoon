use crate::runtime::core::Task;
use crate::{util, Runtime};

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
{
    Runtime::current().expect(util::NO_RUNTIME).spawn(future)
}

pub struct JoinHandle<T> {
    task: Task,
    _t: PhantomData<T>,
}

impl<T> JoinHandle<T> {
    /// # Safety
    ///
    /// `T` must be the return type of the spawned future.
    pub(crate) unsafe fn new(task: Task) -> JoinHandle<T> {
        JoinHandle {
            task,
            _t: PhantomData,
        }
    }

    pub fn cancel(self) -> Option<T> {
        unsafe { self.task.cancel::<T>() }
    }
}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe { Pin::new(&mut self.task).poll::<T>(cx) }
    }
}
