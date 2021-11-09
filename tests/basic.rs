use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

#[test]
fn it_works() {
    platoon::block_on(async move {
        let x = Rc::new(Cell::new(0));

        let handles = (0..100)
            .map(|_| {
                let x = x.clone();
                platoon::spawn(async move {
                    platoon::sleep(Duration::from_millis(10)).await;
                    let val = x.get();
                    x.set(x.get() + 1);
                    val
                })
            })
            .collect::<Vec<_>>();

        for (i, handle) in handles.into_iter().enumerate() {
            assert_eq!(handle.await, i);
        }
    });
}

#[test]
#[should_panic(expected = "oh no!")]
fn panic() {
    platoon::block_on(async {
        platoon::spawn(async {
            panic!("oh no!");
        })
        .await;
    });
}
