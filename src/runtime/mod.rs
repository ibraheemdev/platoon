pub(crate) mod core;
pub(crate) mod new;

use self::core::Core;
use crate::task::JoinHandle;

use std::cell::RefCell;
use std::future::Future;
use std::io;
use std::marker::PhantomData;

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = RefCell::new(None);
}

#[derive(Clone)]
pub struct Runtime {
    pub(crate) core: Core,
}

impl Runtime {
    pub fn new() -> io::Result<Self> {
        Core::new().map(|core| Self { core })
    }

    #[must_use = "Creating and immediately dropping an enter guard does nothing"]
    pub fn enter(&self) -> impl Drop + '_ {
        struct EnterGuard(Option<Runtime>);

        impl Drop for EnterGuard {
            fn drop(&mut self) {
                RUNTIME.with(|rt| {
                    *rt.borrow_mut() = self.0.take();
                });
            }
        }

        let old = RUNTIME.with(|rt| rt.borrow_mut().replace(self.clone()));

        EnterGuard(old)
    }

    pub fn current() -> Option<Self> {
        RUNTIME.with(|rt| rt.borrow().clone())
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
    {
        unsafe { JoinHandle::new(self.core.spawn(future)) }
    }

    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        self.core.block_on(async move {
            let _enter = self.enter();
            future.await
        })
    }
}

pub fn block_on<F>(future: F) -> io::Result<F::Output>
where
    F: Future,
{
    Ok(Runtime::new()?.block_on(future))
}