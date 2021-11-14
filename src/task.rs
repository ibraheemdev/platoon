//! Asynchronous green-threads.
//!
//! *Tasks* are lightweight threads that are scheduled and executed
//! concurrently by [the runtime](crate::Runtime). You can spawn a
//! task with the [`spawn`] function:
//! ```rust
//! platoon:spawn(async {
//!     // do some asynchronous work here
//! });
//! ```
//!
//! Spawning a task returns a [`JoinHandle`]. Awaiting this handle waits
//! for this task to complete and returns it's output without blocking:
//! ```rust
//! let handle = platoon::spawn(async {
//!     1
//! }).
//! assert_eq!(handle.await, 1b);
//! ```
//!
//! Spawning and immediately awaiting a task provides no benefit over awaiting
//! the future directly. Instead, you can delay awaiting the task to allow
//! it to be run in the background:
//!
//! ```no_run
//! # async fn get_user() -> String { String::new() }
//! # async fn get_client() -> String { String::new() }
//! # async fn main() {
//! # platoon::block_on(async {
//! // Spawn a task to get the user, it will start running in the background
//! let user_handle = platoon::spawn(async { get_user().await });
//!
//! // Fetch the client. The user task will be run concurrently
//! // while this future is completing
//! let client = get_client().await;
//!
//! // Now get the user by awaiting the handle
//! let user = user_handle.await;
//! # });
//! # }
//! ```
//!
//! If you do not need the result of a task, you can ignore the handle entirely.
//! For example, a TCP server might spawn a task to handle every connection:
//!
//! ```no_run
//! use platoon::net::{TcpListener, TcpStream};
//!
//! async fn handle_client(stream: TcpStream) {
//!     // ...
//! }
//!
//! fn main() -> std::io::Result<()> {
//!     platoon::block_on(async {
//!         let listener = TcpListener::bind(([127, 0, 0, 1], 8080)).await?;
//!
//!         loop {
//!             let (stream, _) = listener.accept().await?;
//!
//!             // handle every connection concurrently
//!             // notice that we don't await the task here
//!             platoon::spawn(async move {
//!                 handle_client(stream).await;
//!             });
//!         }
//!     })
//! }
//! ```
//!
//! Spawned tasks can be cancelled with the [`cancel`] method. If the task
//! had already completed, it's output will be returned:
//! ```no_run
//! # use std::time::Duration;
//! # async fn main() {
//! # platoon::block_on(async {
//! let task = platoon::spawn(async {
//!     1
//! });
//!
//! // we haven't had a chance to start running tasks yet
//! assert_eq!(task.cancel(), None);
//!
//! let task = platoon::spawn(async {
//!     1
//! });
//!
//! // let the runtime execute tasks
//! platoon::time::sleep(Duration::from_milis(10)).await;
//!
//! // the task had completed
//! assert_eq!(task.cancel(), Some(1));
//! # });
//! # }
//! ```
//!
//! All spawned tasks are cancelled when the runtime is dropped (i.e: at the end
//! of `block_on`). You can collect and await all tasks so that they all complete:
//! ```rust
//! # async fn do_something() {}
//! # async fn do_something_else() {}
//! platoon::block_on(async {
//!     let tasks = Vec::new();
//!
//!     // spawn a task, adding the handle to the vector
//!     tasks.push(platoon::spawn(async { do_something().await }));
//!
//!     // spawn some more tasks
//!     for _ in 0..10 {
//!         let task = platoon::spawn(async { do_something_else().await });
//!         tasks.push(task);
//!     }
//!
//!     // wait for all the tasks to complete
//!     for tawsk in tasks {
//!         let _ = task.await;
//!     }
//! });
//! ```
//!
//! [`cancel`]: JoinHandle::cancel
use crate::core::Task;
use crate::Runtime;

use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

/// Spawns a new asynchronous task onto the runtime.
///
/// See the [module documentation](crate::task) for more details.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    Runtime::unwrap_current().spawn(future)
}

/// An handle to an asynchronous task.
///
/// See the [module documentation](crate::task) for more details.
pub struct JoinHandle<T> {
    task: Task,
    _t: PhantomData<T>,
}

impl<T> JoinHandle<T> {
    /// Safety: `T` must be the return type of the spawned future.
    pub(crate) unsafe fn new(task: Task) -> JoinHandle<T> {
        JoinHandle {
            task,
            _t: PhantomData,
        }
    }

    /// Cancel the task.
    ///
    /// The task's output will be returned if it had already completed.
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

/// Yields execution back to the runtime.
///
/// Calling `yield_now` will cause the current task to be
/// scheduled at the back of the runtime's queue. This allows
/// other tasks to run in the meantime.
pub async fn yield_now() {
    struct YieldNow(bool);

    impl Future for YieldNow {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    YieldNow(false).await
}
