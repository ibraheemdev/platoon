//! Synchronization primitives.
//!
//! Like threads, concurrent [tasks](crate::task) often need to communicate
//! with each other. This module contains abstractions for doing so.
//!
//! Note that all the types in this module are `!Send`, and as such do not
//! have any multi-threaded synchronization overhead.
pub mod oneshot;

mod mutex;
mod rwlock;
mod semaphore;

pub use mutex::{Mutex, MutexGuard};
pub use rwlock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
pub use semaphore::{Permit, Semaphore};
