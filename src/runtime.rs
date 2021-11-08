use crate::core::Core;
use crate::task::JoinHandle;
use crate::util::LocalCell;

use std::future::Future;
use std::io;

thread_local! {
    static RUNTIME: LocalCell<Option<Runtime>> = LocalCell::new(None);
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
                    rt.replace(self.0.take());
                });
            }
        }

        let old = RUNTIME.with(|rt| rt.replace(Some(self.clone())));

        EnterGuard(old)
    }

    pub fn current() -> Option<Self> {
        RUNTIME.with(LocalCell::cloned)
    }

    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
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

pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    Runtime::new()
        .expect("failed to create runtime")
        .block_on(future)
}
