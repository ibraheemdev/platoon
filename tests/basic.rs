use std::cell::Cell;
use std::rc::Rc;

#[test]
fn it_works() {
    platoon::block_on(async move {
        let mut handles = Vec::with_capacity(100);

        let x = Rc::new(Cell::new(0));

        for _ in 0..100 {
            let x = x.clone();
            let h = platoon::spawn(async move {
                platoon::time::sleep(std::time::Duration::from_millis(10)).await;
                let val = x.get();
                x.set(x.get() + 1);
                val
            });
            handles.push(h);
        }

        for (i, handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.await, i);
        }
    });
}
