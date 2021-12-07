use platoon::sync::Semaphore;
use std::rc::Rc;
use tokio_test::task::spawn;
use tokio_test::{assert_pending, assert_ready};

// #[test]
// fn no_permits() {
//     Semaphore::new(0);
// }
// 
// #[test]
// fn fairness() {
//     let sem = Semaphore::new(10);
// 
//     let mut t1 = spawn(sem.acquire(6));
//     let _g1 = assert_ready!(t1.poll());
// 
//     let mut t2 = spawn(sem.acquire(2));
//     let _g2 = assert_ready!(t2.poll());
// 
//     let mut t3 = spawn(sem.acquire(100));
//     assert_pending!(t3.poll());
// 
//     let mut t4 = spawn(sem.acquire(2));
//     assert_pending!(t4.poll());
// 
//     let mut t5 = spawn(sem.acquire(2));
//     assert_pending!(t5.poll());
// 
//     let mut t6 = spawn(sem.acquire(2));
//     assert_pending!(t6.poll());
// 
//     drop(t3);
// 
//     dbg!(t4.is_woken());
//     dbg!(t5.is_woken());
//     dbg!(t6.is_woken());
//     assert!(t4.is_woken());
//     let g4 = assert_ready!(t4.poll());
// 
//     assert_pending!(t5.poll());
//     assert_pending!(t6.poll());
// 
//     drop(g4);
// 
//     assert!(t5.is_woken());
//     let g5 = assert_ready!(t5.poll());
// 
//     assert_pending!(t6.poll());
// 
//     drop(g5);
// 
//     assert!(t6.is_woken());
//     assert_ready!(t6.poll());
// }
// 
// // #[test]
// // fn wake_all() {
// //     let sem = Semaphore::new(10);
// //
// //     let mut t1 = spawn(sem.acquire(10));
// //     let g1 = assert_ready!(t1.poll());
// //
// //     let mut t2 = spawn(sem.acquire(2));
// //     assert_pending!(t2.poll());
// //     let mut t3 = spawn(sem.acquire(2));
// //     assert_pending!(t3.poll());
// //     let mut t4 = spawn(sem.acquire(2));
// //     assert_pending!(t4.poll());
// //     let mut t5 = spawn(sem.acquire(2));
// //     assert_pending!(t5.poll());
// //     let mut t6 = spawn(sem.acquire(2));
// //     assert_pending!(t6.poll());
// //     let mut t7 = spawn(sem.acquire(1));
// //     assert_pending!(t7.poll());
// //
// //     drop(g1);
// //
// //     assert!(t2.is_woken());
// //
// //     assert!(t3.is_woken());
// //     let _g3 = assert_ready!(t3.poll());
// //     assert!(t4.is_woken());
// //     let _g4 = assert_ready!(t4.poll());
// //     assert!(t5.is_woken());
// //     let _g5 = assert_ready!(t5.poll());
// //     assert!(t6.is_woken());
// //     let _g6 = assert_ready!(t6.poll());
// //
// //     assert!(!t7.is_woken());
// //     assert_pending!(t7.poll());
// //
// //     drop(t2);
// //     assert!(t7.is_woken());
// //     assert_ready!(t7.poll());
// // }
// 
// #[test]
// fn try_acquire() {
//     let sem = Semaphore::new(1);
//     {
//         let p1 = sem.try_acquire(1);
//         assert!(p1.is_some());
//         let p2 = sem.try_acquire(1);
//         assert!(p2.is_none());
//     }
//     let p3 = sem.try_acquire(1);
//     assert!(p3.is_some());
// }
// 
// #[test]
// fn acquire() {
//     platoon::block_on(async {
//         let sem = Rc::new(Semaphore::new(1));
//         let p1 = sem.try_acquire(1).unwrap();
//         let sem_clone = sem.clone();
//         let j = platoon::spawn(async move {
//             let _p2 = sem_clone.acquire(1).await;
//         });
//         drop(p1);
//         j.await;
//     });
// }
// 
// #[test]
// fn add_permits() {
//     platoon::block_on(async {
//         let sem = Rc::new(Semaphore::new(0));
//         let sem_clone = sem.clone();
//         let j = platoon::spawn(async move {
//             let _p2 = sem_clone.acquire(1).await;
//         });
//         sem.add_permits(1);
//         j.await;
//     });
// }
// 
// #[test]
// fn forget() {
//     let sem = Rc::new(Semaphore::new(1));
//     {
//         let p = sem.try_acquire(1).unwrap();
//         assert_eq!(sem.permits(), 0);
//         p.forget();
//         assert_eq!(sem.permits(), 0);
//     }
//     assert_eq!(sem.permits(), 0);
//     assert!(sem.try_acquire(1).is_none());
// }
// 
// #[test]
// fn stresstest() {
//     platoon::block_on(async {
//         let sem = Rc::new(Semaphore::new(5));
//         let mut join_handles = Vec::new();
//         for _ in 0..1000 {
//             let sem_clone = sem.clone();
//             join_handles.push(platoon::spawn(async move {
//                 let _p = sem_clone.acquire(1).await;
//             }));
//         }
//         for j in join_handles {
//             j.await;
//         }
//         // there should be exactly 5 semaphores available now
//         let _p1 = sem.try_acquire(1).unwrap();
//         let _p2 = sem.try_acquire(1).unwrap();
//         let _p3 = sem.try_acquire(1).unwrap();
//         let _p4 = sem.try_acquire(1).unwrap();
//         let _p5 = sem.try_acquire(1).unwrap();
//         assert!(sem.try_acquire(1).is_none());
//     });
// }
// 
// #[test]
// fn add_max_amount_permits() {
//     let s = Semaphore::new(0);
//     s.add_permits(usize::MAX);
//     assert_eq!(s.permits(), usize::MAX);
// }
// 
// #[test]
// #[should_panic]
// fn add_more_than_max_amount_permits() {
//     let s = Semaphore::new(1);
//     s.add_permits(usize::MAX);
// }
// 
// #[test]
// fn try_acquire_many() {
//     let sem = Semaphore::new(42);
//     {
//         let p1 = sem.try_acquire(42);
//         assert!(p1.is_some());
//         let p2 = sem.try_acquire(1);
//         assert!(p2.is_none());
//     }
//     let p3 = sem.try_acquire(32);
//     assert!(p3.is_some());
//     let p4 = sem.try_acquire(10);
//     assert!(p4.is_some());
//     assert!(sem.try_acquire(1).is_none());
// }

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
                dbg!("HERE");
                let _permit32 = semaphore.acquire(32).await;
            }
        });
        receiver.await.unwrap();
        drop(permit32);
        join_handle.await;
    });
}
