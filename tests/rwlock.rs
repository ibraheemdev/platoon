use platoon::sync::RwLock;

use std::rc::Rc;

use futures_util::{stream, FutureExt, StreamExt};
use tokio_test::task::spawn;
use tokio_test::{assert_pending, assert_ready};

#[test]
fn into_inner() {
    let rwlock = RwLock::new(42);
    assert_eq!(rwlock.into_inner(), 42);
}

// multiple reads should be Ready
#[test]
fn read_shared() {
    let rwlock = RwLock::new(100);

    let mut t1 = spawn(rwlock.read());
    let _g1 = assert_ready!(t1.poll());
    let mut t2 = spawn(rwlock.read());
    assert_ready!(t2.poll());
}

// When there is an active shared owner, exclusive access should not be possible
#[test]
fn write_shared_pending() {
    let rwlock = RwLock::new(100);
    let mut t1 = spawn(rwlock.read());

    let _g1 = assert_ready!(t1.poll());
    let mut t2 = spawn(rwlock.write());
    assert_pending!(t2.poll());
}

// When there is an active exclusive owner, subsequent exclusive access should not be possible
#[test]
fn read_exclusive_pending() {
    let rwlock = RwLock::new(100);
    let mut t1 = spawn(rwlock.write());

    let _g1 = assert_ready!(t1.poll());
    let mut t2 = spawn(rwlock.read());
    assert_pending!(t2.poll());
}

// When there is an active exclusive owner, subsequent exclusive access should not be possible
#[test]
fn write_exclusive_pending() {
    let rwlock = RwLock::new(100);
    let mut t1 = spawn(rwlock.write());

    let _g1 = assert_ready!(t1.poll());
    let mut t2 = spawn(rwlock.write());
    assert_pending!(t2.poll());
}

// When there is an active shared owner, exclusive access should be possible after shared is dropped
#[test]
fn write_shared_drop() {
    let rwlock = RwLock::new(100);
    let mut t1 = spawn(rwlock.read());

    let g1 = assert_ready!(t1.poll());
    let mut t2 = spawn(rwlock.write());
    assert_pending!(t2.poll());
    drop(g1);
    assert!(t2.is_woken());
    assert_ready!(t2.poll());
}

// when there is an active shared owner, and exclusive access is triggered,
// subsequent shared access should not be possible as write gathers all the available semaphore permits
#[test]
fn write_read_shared_pending() {
    let rwlock = RwLock::new(100);
    let mut t1 = spawn(rwlock.read());
    let _g1 = assert_ready!(t1.poll());

    let mut t2 = spawn(rwlock.read());
    assert_ready!(t2.poll());

    let mut t3 = spawn(rwlock.write());
    assert_pending!(t3.poll());

    let mut t4 = spawn(rwlock.read());
    assert_pending!(t4.poll());
}

// when there is an active shared owner, and exclusive access is triggered,
// reading should be possible after pending exclusive access is dropped
#[test]
fn write_read_shared_drop_pending() {
    let rwlock = RwLock::new(100);
    let mut t1 = spawn(rwlock.read());
    let _g1 = assert_ready!(t1.poll());

    let mut t2 = spawn(rwlock.write());
    assert_pending!(t2.poll());

    let mut t3 = spawn(rwlock.read());
    assert_pending!(t3.poll());
    drop(t2);

    assert!(t3.is_woken());
    assert_ready!(t3.poll());
}

// Acquire an RwLock nonexclusively by a single task
#[test]
fn read_uncontested() {
    platoon::block_on(async {
        let rwlock = RwLock::new(100);
        let result = *rwlock.read().await;

        assert_eq!(result, 100);
    });
}

// Acquire an uncontested RwLock in exclusive mode
#[test]
fn write_uncontested() {
    platoon::block_on(async {
        let rwlock = RwLock::new(100);
        let mut result = rwlock.write().await;
        *result += 50;
        assert_eq!(*result, 150);
    });
}

// RwLocks should be acquired in the order that their Futures are waited upon.
#[test]
fn write_order() {
    platoon::block_on(async {
        let rwlock = RwLock::<Vec<u32>>::new(vec![]);
        let fut2 = rwlock.write().map(|mut guard| guard.push(2));
        let fut1 = rwlock.write().map(|mut guard| guard.push(1));
        fut1.await;
        fut2.await;

        let g = rwlock.read().await;
        assert_eq!(*g, vec![1, 2]);
    });
}

#[test]
fn stress() {
    platoon::block_on(async {
        let mut tasks = Vec::new();
        let rwlock = Rc::new(RwLock::<u32>::new(0));
        let rwclone1 = rwlock.clone();
        let rwclone2 = rwlock.clone();
        let rwclone3 = rwlock.clone();
        let rwclone4 = rwlock.clone();

        tasks.push(platoon::spawn(async move {
            stream::iter(0..1000)
                .for_each(move |_| {
                    let rwlock = rwclone1.clone();
                    async move {
                        let mut guard = rwlock.write().await;
                        *guard += 2;
                    }
                })
                .await;
        }));

        tasks.push(platoon::spawn(async move {
            stream::iter(0..1000)
                .for_each(move |_| {
                    let rwlock = rwclone2.clone();
                    async move {
                        let mut guard = rwlock.write().await;
                        *guard += 3;
                    }
                })
                .await;
        }));

        tasks.push(platoon::spawn(async move {
            stream::iter(0..1000)
                .for_each(move |_| {
                    let rwlock = rwclone3.clone();
                    async move {
                        let mut guard = rwlock.write().await;
                        *guard += 5;
                    }
                })
                .await;
        }));

        tasks.push(platoon::spawn(async move {
            stream::iter(0..1000)
                .for_each(move |_| {
                    let rwlock = rwclone4.clone();
                    async move {
                        let mut guard = rwlock.write().await;
                        *guard += 7;
                    }
                })
                .await;
        }));

        for task in tasks {
            task.await;
        }

        let g = rwlock.read().await;
        assert_eq!(*g, 17_000);
    });
}

#[test]
fn try_write() {
    platoon::block_on(async {
        let lock = RwLock::new(0);
        let read_guard = lock.read().await;
        assert!(lock.try_write().is_none());
        drop(read_guard);
        assert!(lock.try_write().is_some());
    });
}

#[test]
fn try_read_try_write() {
    let lock: RwLock<usize> = RwLock::new(15);

    {
        let rg1 = lock.try_read().unwrap();
        assert_eq!(*rg1, 15);

        assert!(lock.try_write().is_none());

        let rg2 = lock.try_read().unwrap();
        assert_eq!(*rg2, 15)
    }

    {
        let mut wg = lock.try_write().unwrap();
        *wg = 1515;

        assert!(lock.try_read().is_none())
    }

    assert_eq!(*lock.try_read().unwrap(), 1515);
}
