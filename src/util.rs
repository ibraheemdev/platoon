use std::cell::UnsafeCell;
use std::future::Future;
use std::mem;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

pub async fn poll_fn<T, F>(f: F) -> T
where
    F: FnMut(&mut Context<'_>) -> Poll<T>,
{
    struct PollFn<F>(F);

    impl<F> Unpin for PollFn<F> {}

    impl<T, F> Future for PollFn<F>
    where
        F: FnMut(&mut Context<'_>) -> Poll<T>,
    {
        type Output = T;

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
            (self.0)(cx)
        }
    }

    PollFn(f).await
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

pub struct LocalCell<T> {
    value: UnsafeCell<T>,
    #[cfg(debug_assertions)]
    borrowed: std::cell::Cell<bool>,
}

impl<T> LocalCell<T> {
    pub fn new(value: T) -> LocalCell<T> {
        LocalCell {
            value: UnsafeCell::new(value),
            #[cfg(debug_assertions)]
            borrowed: Default::default(),
        }
    }

    pub fn cloned(&self) -> T
    where
        T: Clone,
    {
        unsafe { self.with(|value| value.clone()) }
    }

    pub fn replace(&self, value: T) -> T {
        unsafe { self.with(|old| mem::replace(old, value)) }
    }

    /// # Safety
    ///
    /// `with` must not be called again within the closure.
    pub unsafe fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        #[cfg(debug_assertions)]
        if self.borrowed.replace(true) {
            panic!("attempted to borrow LocalCell twice");
        }

        // SAFETY:
        // - caller guarantees that `with` will
        //  not be called in `f`, and that is the only
        //  way to get a reference to `val`.
        // - LocalCell is !Sync
        let val = unsafe { &mut *self.value.get() };
        let val = f(val);

        #[cfg(debug_assertions)]
        self.borrowed.set(false);

        val
    }
}

macro_rules! syscall {
    ($fn:ident $args:tt) => {{
        let res = unsafe { libc::$fn $args };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

pub(crate) use syscall;
