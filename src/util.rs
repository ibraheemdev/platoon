use std::cell::UnsafeCell;
// use std::thread::LocalKey;
use std::future::Future;
use std::hash::Hasher;
use std::mem::{self, ManuallyDrop};
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};

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

/// `RcWake` is similar to `Wake`, except wakers
/// are reference counted through `Rc` instead of
/// `Arc`.
///
/// # Safety
///
/// `created_on` must return the `ThreadId` of thread that the
/// waker was created on.
pub unsafe trait RcWake: 'static {
    fn wake(self: Rc<Self>);
    fn created_on(&self) -> ThreadId;

    fn wake_by_ref(self: &Rc<Self>) {
        self.clone().wake()
    }

    fn into_waker(self: Rc<Self>) -> Waker
    where
        Self: Sized,
    {
        unsafe fn assert_not_sent(waker: &Rc<impl RcWake>) {
            if thread::current().id() != waker.created_on() {
                panic!("cannot use waker from outside the thread it was created on");
            }
        }

        unsafe fn clone<W: RcWake>(waker: *const ()) -> RawWaker {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            mem::forget(waker.clone());

            RawWaker::new(
                Rc::into_raw(waker) as *const (),
                &RawWakerVTable::new(clone::<W>, wake::<W>, wake_by_ref::<W>, drop::<W>),
            )
        }

        unsafe fn wake<W: RcWake>(waker: *const ()) {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            W::wake(waker);
        }

        unsafe fn wake_by_ref<W: RcWake>(waker: *const ()) {
            let waker = unsafe { ManuallyDrop::new(Rc::from_raw(waker as *const W)) };
            assert_not_sent(&waker);
            W::wake_by_ref(&waker);
        }

        unsafe fn drop<W: RcWake>(waker: *const ()) {
            let waker = unsafe { Rc::from_raw(waker as *const W) };
            assert_not_sent(&waker);
            let _ = waker;
        }

        let raw = RawWaker::new(
            Rc::into_raw(self) as *const (),
            &RawWakerVTable::new(
                clone::<Self>,
                wake::<Self>,
                wake_by_ref::<Self>,
                drop::<Self>,
            ),
        );

        unsafe { Waker::from_raw(raw) }
    }
}

pub struct LocalCell<T> {
    value: UnsafeCell<T>,
}

impl<T> LocalCell<T> {
    pub fn new(value: T) -> LocalCell<T> {
        LocalCell {
            value: UnsafeCell::new(value),
        }
    }

    /// # Safety
    ///
    /// `with` must not be called again within the closure.
    pub unsafe fn with<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        // SAFETY:
        // - caller guarantees that `with` will
        //  not be called in `f`, and that is the only
        //  way to get a reference to `val`.
        // - LocalCell is !Sync
        let value = unsafe { &mut *self.value.get() };
        f(value)
    }
}
