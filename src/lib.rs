#![allow(unused_unsafe)]

pub mod runtime;
pub mod task;
pub mod time;

mod util;

pub use runtime::Runtime;
pub use task::spawn;

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
                time::sleep(std::time::Duration::from_millis(10)).await;
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
