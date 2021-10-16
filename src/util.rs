use std::future::Future;
use std::hash::Hasher;
use std::pin::Pin;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

pub async fn poll_fn<T, F>(f: F) -> T
where
    F: FnMut(&mut Context<'_>) -> Poll<T>,
{
    struct PollFn<F> {
        f: F,
    }

    impl<F> Unpin for PollFn<F> {}

    impl<T, F> Future for PollFn<F>
    where
        F: FnMut(&mut Context<'_>) -> Poll<T>,
    {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
            (&mut self.f)(cx)
        }
    }

    PollFn { f }.await
}

pub const NO_RUNTIME: &str = "must be called within the context of the Astra runtime";

#[derive(Default)]
pub struct UsizeHasher(usize);

impl Hasher for UsizeHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!()
    }

    #[inline]
    fn write_usize(&mut self, id: usize) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0 as _
    }
}

pub fn noop_waker() -> Waker {
    fn noop_raw_waker() -> RawWaker {
        RawWaker::new(
            std::ptr::null(),
            &RawWakerVTable::new(|_| noop_raw_waker(), |_| (), |_| (), |_| ()),
        )
    }

    unsafe { Waker::from_raw(noop_raw_waker()) }
}

pub fn wake(waker: Waker) {
    #[cfg(debug_assertions)]
    {
        waker.wake();
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = std::panic::catch_unwind(|| waker.wake());
    }
}
