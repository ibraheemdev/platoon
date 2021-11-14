//! A channel for sending a single message between tasks.
//!
//! # Examples
//!
//! ```
//! use platoon::sync::oneshot;
//!
//! # platoon::block_on(async {
//! let (tx, rx) = oneshot::channel();
//!
//! platoon::spawn(async move {
//!     if let Err(_) = tx.send("hello") {
//!         panic!("receiver dropped");
//!     }
//! });
//!
//! match rx.await {
//!     Ok(msg) => println!("{}", msg),
//!     Err(_) => panic!("sender dropped"),
//! }
//! # });
//! ```
use crate::util;

use std::cell::Cell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

/// Creates a new oneshot channel.
///
/// See [the module documentation](crate::sync::oneshot) for details.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let inner = Rc::new(Shared {
        complete: Default::default(),
        message: Default::default(),
        sender: Default::default(),
        receiver: Default::default(),
    });

    (Sender(inner.clone()), Receiver(inner))
}

/// The sender half of a oneshot channel.
pub struct Sender<T>(Rc<Shared<T>>);

/// The receiver half of a oneshot channel.
pub struct Receiver<T>(Rc<Shared<T>>);

struct Shared<T> {
    complete: Cell<bool>,
    message: Cell<Option<T>>,
    sender: Cell<Option<Waker>>,
    receiver: Cell<Option<Waker>>,
}

impl<T> Sender<T> {
    /// Send a message on this channel.
    ///
    /// If the receiver was already dropped, the message is returned as an
    /// error.
    pub fn send(self, message: T) -> Result<(), T> {
        if self.0.complete.get() {
            Err(message)
        } else {
            self.0.message.set(Some(message));
            Ok(())
        }
    }

    /// Checks whether the associated [`Receiver`] has been closed.
    ///
    /// If the receiver was not dropped, then `Poll::Pending` is
    /// returned and the current task will be woken up when it is.
    pub fn poll_closed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.0.complete.get() {
            Poll::Ready(())
        } else {
            self.0.sender.set(Some(cx.waker().clone()));
            Poll::Pending
        }
    }

    /// Waits for the associated [`Receiver]` to close.
    pub async fn closed(&mut self) {
        util::poll_fn(|cx| self.poll_closed(cx)).await
    }

    /// Returns true if the associated [`Receiver`] has been closed.
    pub fn is_closed(&self) -> bool {
        self.0.complete.get()
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.0.complete.set(true);
        if let Some(waker) = self.0.receiver.take() {
            waker.wake();
        }
        self.0.sender.take();
    }
}

impl<T> Receiver<T> {
    /// Close this oneshot channel.
    ///
    /// After this method is called any attempts to [`send`](Sender::send)
    /// a message on this channel will fail.
    pub fn close(&mut self) {
        self.0.complete.set(true);
        if let Some(waker) = self.0.sender.take() {
            waker.wake();
        }
    }

    /// Attempts to receive a message.
    ///
    /// The current task is *not* scheduled to be woken up
    /// if no message is available.
    pub fn try_recv(&mut self) -> Result<Option<T>, Closed> {
        if self.0.complete.get() {
            match self.0.message.take() {
                Some(message) => Ok(Some(message)),
                None => Err(Closed),
            }
        } else {
            Ok(None)
        }
    }
}

impl<T> Future for Receiver<T> {
    type Output = Result<T, Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<T, Closed>> {
        if self.0.complete.get() {
            if let Some(message) = self.0.message.take() {
                Poll::Ready(Ok(message))
            } else {
                Poll::Ready(Err(Closed))
            }
        } else {
            self.0.receiver.set(Some(cx.waker().clone()));
            Poll::Pending
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.0.complete.set(true);
        self.0.receiver.take();

        if let Some(waker) = self.0.sender.take() {
            waker.wake();
        }
    }
}

/// Error returned by a [`Receiver`] if the channel
/// has been closed (i.e: the associated sender has
/// been dropped);
pub struct Closed;
