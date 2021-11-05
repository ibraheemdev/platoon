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
    pub(crate) task: Task,
    pub(crate) _t: PhantomData<T>,
}

impl<T> Unpin for JoinHandle<T> {}

impl<T> Future for JoinHandle<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.task)
            .poll(cx)
            .map(|ptr| unsafe { *Box::from_raw(ptr as *mut T) })
    }
}
