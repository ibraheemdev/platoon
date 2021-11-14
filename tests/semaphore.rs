// TODO: test with mpsc channel
use platoon::sync::Semaphore;

use std::rc::Rc;

#[test]
fn try_acquire() {
    let s = Semaphore::new(2);
    let g1 = s.try_acquire(1).unwrap();
    let _g2 = s.try_acquire(1).unwrap();

    assert!(s.try_acquire(1).is_none());
    drop(g1);
    assert!(s.try_acquire(1).is_some());
}

#[test]
fn stress() {
    platoon::block_on(async {
        let s = Rc::new(Semaphore::new(5));

        let handles = (0..50)
            .map(|_| {
                let s = s.clone();
                platoon::spawn(async move {
                    for _ in 0..10_000 {
                        s.acquire(1).await;
                    }
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.await;
        }

        let _g1 = s.try_acquire(1).unwrap();
        let g2 = s.try_acquire(1).unwrap();
        let _g3 = s.try_acquire(1).unwrap();
        let _g4 = s.try_acquire(1).unwrap();
        let _g5 = s.try_acquire(1).unwrap();

        assert!(s.try_acquire(1).is_none());
        drop(g2);
        assert!(s.try_acquire(1).is_some());
    });
}

#[test]
fn mutex() {
    platoon::block_on(async {
        let s = Rc::new(Semaphore::new(1));
        let s2 = s.clone();
        let _t = platoon::spawn(async move {
            let _g = s2.acquire(1).await;
        });
        let _g = s.acquire(1).await;
    });
}
