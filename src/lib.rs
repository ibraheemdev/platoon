#![deny(unsafe_op_in_unsafe_fn)]

pub mod net;
pub mod runtime;
pub mod task;
pub mod time;

mod core;
mod sys;
mod util;

#[doc(inline)]
pub use self::{
    runtime::{block_on, Runtime},
    task::spawn,
    time::sleep,
};
