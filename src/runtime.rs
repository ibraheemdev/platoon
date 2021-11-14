use crate::core::Core;
use crate::task::JoinHandle;
use crate::util::LocalCell;

use std::future::Future;
use std::io;

thread_local! {
    static RUNTIME: LocalCell<Option<Runtime>> = LocalCell::new(None);
}

/// The platoon runtime.
///
/// A runtime is created implicitly with the [`block_on`] method. Functions
/// like [`spawn`] and [`TcpStream::connect`] rely on the thread-local runtime
/// *context*. Creating a runtime manually is useful if you don't want to rely
/// on this context, such as if you want to have multiple runtimes on the same
/// thread or want to embed the runtime in a independent type.
///
/// [`spawn`]: crate::spawn
/// [`TcpStream::connect`]: crate::net::TcpStream::connect
#[derive(Clone)]
pub struct Runtime {
    pub(crate) core: Core,
}

impl Runtime {
    /// Create a new runtime.
    pub fn new() -> io::Result<Self> {
        Core::new().map(|core| Self { core })
    }

    /// Enter the runtime context.
    ///
    /// The returned guard will exit the context when it is dropped.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let runtime = platoon::Runtime::new().unwrap();
    /// {
    ///     let _enter = runtime.enter();
    ///     // spawn relies on the runtime context
    ///     platoon::spawn(async { });
    /// }
    ///
    /// // this would panic: platoon::spawn(...)
    /// ```
    #[must_use = "Creating and immediately dropping an enter guard does nothing"]
    pub fn enter(&self) -> impl Drop + '_ {
        struct Guard(Option<Runtime>);

        impl Drop for Guard {
            fn drop(&mut self) {
                RUNTIME.with(|rt| {
                    rt.replace(self.0.take());
                });
            }
        }

        let old = RUNTIME.with(|rt| rt.replace(Some(self.clone())));

        Guard(old)
    }

    /// Returns the current runtime if set.
    pub fn current() -> Option<Self> {
        RUNTIME.with(LocalCell::cloned)
    }

    pub(crate) fn unwrap_current() -> Self {
        Self::current().expect("must be called within the context of the platoon runtime")
    }

    /// Spawns a task onto the runtime.
    ///
    /// This method is equivalent to [`spawn`](crate::spawn) except
    /// it uses this runtime instead of relying on the thread-local
    /// context.
    pub fn spawn<F>(&self, future: F) -> JoinHandle<F::Output>
    where
        F: Future + 'static,
        F::Output: 'static,
    {
        unsafe { JoinHandle::new(self.core.spawn(future)) }
    }

    /// Runs the provided future to completion.
    ///
    /// This method is equivalent to [`block_on`] except it uses
    /// this runtime instead of relying on the thread-local
    /// context.
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

/// Runs the provided future to completion.
///
/// Any tasks, timers, and I/O or be run concurrently with this future
/// until it completes.
///
/// # Examples
///
/// ```no_run
/// use std::time::Duration;
///
/// fn main() {
///     platoon::block_on(async {
///         platoon::sleep(Duration::from_secs(1)).await;
///         println!("Hello world!");
///     });
/// }
/// ```
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future,
{
    Runtime::new()
        .expect("failed to create runtime")
        .block_on(future)
}
