use platoon::sync::Mutex;
use platoon::time::{interval, timeout};

use std::sync::Arc;
use std::time::Duration;

use tokio_test::task::spawn;
use tokio_test::{assert_pending, assert_ready};

#[test]
fn straight_execution() {
    let l = Mutex::new(100);

    {
        let mut t = spawn(l.lock());
        let mut g = assert_ready!(t.poll());
        assert_eq!(&*g, &100);
        *g = 99;
    }
    {
        let mut t = spawn(l.lock());
        let mut g = assert_ready!(t.poll());
        assert_eq!(&*g, &99);
        *g = 98;
    }
    {
        let mut t = spawn(l.lock());
        let g = assert_ready!(t.poll());
        assert_eq!(&*g, &98);
    }
}

#[test]
fn readiness() {
    let l1 = Arc::new(Mutex::new(100));
    let l2 = Arc::clone(&l1);
    let mut t1 = spawn(l1.lock());
    let mut t2 = spawn(l2.lock());

    let g = assert_ready!(t1.poll());

    // We can't now acquire the lease since it's already held in g
    assert_pending!(t2.poll());

    // But once g unlocks, we can acquire it
    drop(g);
    assert!(t2.is_woken());
    assert_ready!(t2.poll());
}

// Ensure a mutex is unlocked if a future holding the lock
// is aborted prematurely.
#[test]
fn aborted_future_1() {
    platoon::block_on(async {
        let m1: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        {
            let m2 = m1.clone();
            // Try to lock mutex in a future that is aborted prematurely
            timeout(Duration::from_millis(1u64), async move {
                let mut i = interval(Duration::from_millis(1000));
                m2.lock().await;
                i.tick().await;
                i.tick().await;
            })
            .await
            .unwrap_err();
        }
        // This should succeed as there is no lock left for the mutex.
        timeout(Duration::from_millis(1u64), async move {
            m1.lock().await;
        })
        .await
        .expect("Mutex is locked");
    });
}

// This test is similar to `aborted_future_1` but this time the
// aborted future is waiting for the lock.
#[test]
fn aborted_future_2() {
    platoon::block_on(async {
        let m1: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
        {
            // Lock mutex
            let _lock = m1.lock().await;
            {
                let m2 = m1.clone();
                // Try to lock mutex in a future that is aborted prematurely
                timeout(Duration::from_millis(1u64), async move {
                    m2.lock().await;
                })
                .await
                .unwrap_err();
            }
        }
        // This should succeed as there is no lock left for the mutex.
        timeout(Duration::from_millis(1u64), async move {
            m1.lock().await;
        })
        .await
        .expect("Mutex is locked");
    });
}

#[test]
fn try_lock() {
    let m: Mutex<usize> = Mutex::new(0);
    {
        let g1 = m.try_lock();
        assert!(g1.is_some());
        let g2 = m.try_lock();
        assert!(!g2.is_some());
    }
    let g3 = m.try_lock();
    assert!(g3.is_some());
}
