pub(crate) mod core;
pub(crate) mod shared;

use self::core::{Core, JoinHandle};
use self::shared::Shared;

use std::cell::{RefCell, UnsafeCell};
use std::future::Future;
use std::io;
use std::rc::Rc;

thread_local! {
    static RUNTIME: RefCell<Option<Runtime>> = RefCell::new(None);
}

#[derive(Clone)]
pub struct Runtime {
    pub(crate) shared: Shared,
}

impl Runtime {
    pub fn new() -> io::Result<Self> {
        Core::new().map(|core| Self {
            shared: Shared {
                core: Rc::new(UnsafeCell::new(core)),
            },
        })
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
        self.shared.spawn(future)
    }

    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        self.shared.block_on(async move {
            let _enter = self.enter();
            future.await
        })
    }
}
