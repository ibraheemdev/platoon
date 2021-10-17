#[doc(inline)]
pub use crate::runtime::core::JoinHandle;

use crate::{util, Runtime};

use std::future::Future;

pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + 'static,
{
    Runtime::current().expect(util::NO_RUNTIME).spawn(future)
}
