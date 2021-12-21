#![deny(unsafe_op_in_unsafe_fn)]
#![allow(dead_code)]

pub mod net;
pub mod sync;
pub mod task;
pub mod time;

mod runtime;
mod sys;
mod util;

#[doc(inline)]
pub use self::{
    runtime::{block_on, Runtime},
    task::spawn,
    time::sleep,
};

pub use futures_io as io;

#[macro_export]
macro_rules! pin {
    ($x:ident) => {
        // Move the value to ensure that it is owned
        let mut $x = $x;
        // Shadow the original binding so that it can't be directly accessed
        // ever again.
        #[allow(unused_mut)]
        let mut $x = unsafe { ::std::pin::Pin::new_unchecked(&mut $x) };
    };
}
