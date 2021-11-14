//! Synchronization primitives.
//!
//! Like threads, concurrent [tasks](crate::task) often need to communicate
//! with each other. This module contains abstractions for doing so.
//!
//! Note that all the types in this module are `!Send`, and as such do not
//! have any multi-threaded synchronization overhead.
pub mod oneshot;
