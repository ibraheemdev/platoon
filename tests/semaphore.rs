use platoon::sync::Semaphore;
use std::rc::Rc;

#[test]
fn no_permits() {
    // this should not panic
    Semaphore::new(0);
}

#[test]
fn try_acquire() {
    let sem = Semaphore::new(1);
    {
        let p1 = sem.try_acquire(1);
        assert!(p1.is_some());
        let p2 = sem.try_acquire(1);
        assert!(p2.is_none());
    }
    let p3 = sem.try_acquire(1);
    assert!(p3.is_some());
}

#[test]
fn acquire() {
    platoon::block_on(async {
        let sem = Rc::new(Semaphore::new(1));
        let p1 = sem.try_acquire(1).unwrap();
        let sem_clone = sem.clone();
        let j = platoon::spawn(async move {
            let _p2 = sem_clone.acquire(1).await;
        });
        drop(p1);
        j.await;
    });
}

#[test]
fn add_permits() {
    platoon::block_on(async {
        let sem = Rc::new(Semaphore::new(0));
        let sem_clone = sem.clone();
        let j = platoon::spawn(async move {
            let _p2 = sem_clone.acquire(1).await;
        });
        sem.add_permits(1);
        j.await;
    });
}

#[test]
fn forget() {
    let sem = Rc::new(Semaphore::new(1));
    {
        let p = sem.try_acquire(1).unwrap();
        assert_eq!(sem.permits(), 0);
        p.forget();
        assert_eq!(sem.permits(), 0);
    }
    assert_eq!(sem.permits(), 0);
    assert!(sem.try_acquire(1).is_none());
}

#[test]
fn stresstest() {
    platoon::block_on(async {
        let sem = Rc::new(Semaphore::new(5));
        let mut join_handles = Vec::new();
        for _ in 0..1000 {
            let sem_clone = sem.clone();
            join_handles.push(platoon::spawn(async move {
                let _p = sem_clone.acquire(1).await;
            }));
        }
        for j in join_handles {
            j.await;
        }
        // there should be exactly 5 semaphores available now
        let _p1 = sem.try_acquire(1).unwrap();
        let _p2 = sem.try_acquire(1).unwrap();
        let _p3 = sem.try_acquire(1).unwrap();
        let _p4 = sem.try_acquire(1).unwrap();
        let _p5 = sem.try_acquire(1).unwrap();
        assert!(sem.try_acquire(1).is_none());
    });
}

#[test]
fn add_max_amount_permits() {
    let s = Semaphore::new(0);
    s.add_permits(usize::MAX);
    assert_eq!(s.permits(), usize::MAX);
}

#[test]
#[should_panic]
fn add_more_than_max_amount_permits() {
    let s = Semaphore::new(1);
    s.add_permits(usize::MAX);
}

#[test]
fn try_acquire_many() {
    let sem = Semaphore::new(42);
    {
        let p1 = sem.try_acquire(42);
        assert!(p1.is_some());
        let p2 = sem.try_acquire(1);
        assert!(p2.is_none());
    }
    let p3 = sem.try_acquire(32);
    assert!(p3.is_some());
    let p4 = sem.try_acquire(10);
    assert!(p4.is_some());
    assert!(sem.try_acquire(1).is_none());
}

#[test]
fn acquire_many() {
    platoon::block_on(async {
        let semaphore = Rc::new(Semaphore::new(42));
        let permit32 = semaphore.try_acquire(32).unwrap();
        let (sender, receiver) = platoon::sync::oneshot::channel();

        let join_handle = platoon::spawn({
            let semaphore = semaphore.clone();
            async move {
                let semaphore = semaphore.clone();
                let _permit10 = semaphore.acquire(10).await;
                sender.send(()).unwrap();
                let _permit32 = semaphore.acquire(32).await;
            }
        });
        receiver.await.unwrap();
        drop(permit32);
        join_handle.await;
    });
}
