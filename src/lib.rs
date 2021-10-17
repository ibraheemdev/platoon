#![allow(unused_unsafe)]

mod clock;
mod executor;
mod reactor;
mod runtime;
mod util;

pub use clock::{
    interval, interval_at, sleep, sleep_until, timeout, timeout_at, Interval, Sleep, TimedOut,
    Timeout,
};
pub use runtime::{spawn, Runtime};

#[test]
fn it_works() -> std::io::Result<()> {
    use std::{cell::Cell, rc::Rc};

    let rt = Runtime::new()?;
    rt.block_on(async move {
        let mut handles = vec![];
        let x = Rc::new(Cell::new(0));
        for _ in 0..100 {
            let x = x.clone();
            let h = spawn(async move {
                sleep(std::time::Duration::from_millis(10)).await;
                x.set(x.get() + 1);
                x.get()
            });
            handles.push(h);
        }

        for handle in handles {
            dbg!(handle.await);
        }
    });

    Ok(())
}
