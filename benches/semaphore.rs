use criterion::Criterion;
use platoon::sync::Semaphore;
use std::rc::Rc;

fn uncontended_tokio(c: &mut Criterion) {
    let rt = platoon::Runtime::new().unwrap();
    let s = Rc::new(tokio::sync::Semaphore::new(10));

    c.bench_function("uncontended_tokio", |b| {
        b.iter(|| {
            let s = s.clone();
            rt.block_on(async move {
                for _ in 0..6 {
                    let permit = s.acquire().await;
                    drop(permit);
                }
            })
        });
    });
}

fn uncontended(c: &mut Criterion) {
    let rt = platoon::Runtime::new().unwrap();
    let s = Rc::new(Semaphore::new(10));

    c.bench_function("uncontended", |b| {
        b.iter(|| {
            let s = s.clone();
            rt.block_on(async move {
                for _ in 0..6 {
                    let permit = s.acquire(1).await;
                    drop(permit);
                }
            })
        });
    });
}

async fn tokio_task(s: Rc<tokio::sync::Semaphore>) {
    let permit = s.acquire().await;
    drop(permit);
}

async fn task(s: Rc<Semaphore>) {
    let permit = s.acquire(1).await;
    drop(permit);
}

fn uncontended_concurrent_tokio(c: &mut Criterion) {
    let rt = platoon::Runtime::new().unwrap();
    let s = Rc::new(tokio::sync::Semaphore::new(10));

    c.bench_function("uncontended_concurrent_tokio", |b| {
        b.iter(|| {
            let s = s.clone();
            rt.block_on(async move {
                futures_util::join! {
                    platoon::spawn(tokio_task(s.clone())),
                    platoon::spawn(tokio_task(s.clone())),
                    platoon::spawn(tokio_task(s.clone())),
                    platoon::spawn(tokio_task(s.clone())),
                    platoon::spawn(tokio_task(s.clone())),
                    platoon::spawn(tokio_task(s.clone()))
                };
            })
        });
    });
}

fn uncontended_concurrent(c: &mut Criterion) {
    let rt = platoon::Runtime::new().unwrap();
    let s = Rc::new(Semaphore::new(10));

    c.bench_function("uncontended_concurrent", |b| {
        b.iter(|| {
            let s = s.clone();
            rt.block_on(async move {
                futures_util::join! {
                    platoon::spawn(task(s.clone())),
                    platoon::spawn(task(s.clone())),
                    platoon::spawn(task(s.clone())),
                    platoon::spawn(task(s.clone())),
                    platoon::spawn(task(s.clone())),
                    platoon::spawn(task(s.clone()))
                };
            })
        });
    });
}

fn contended_concurrent_tokio(c: &mut Criterion) {
    let rt = platoon::Runtime::new().unwrap();
    let s = Rc::new(tokio::sync::Semaphore::new(5));

    c.bench_function("contended_concurrent_tokio", |b| {
        b.iter(|| {
            let s = s.clone();
            rt.block_on(async move {
                futures_util::join! {
                    tokio_task(s.clone()),
                    tokio_task(s.clone()),
                    tokio_task(s.clone()),
                    tokio_task(s.clone()),
                    tokio_task(s.clone()),
                    tokio_task(s.clone())
                };
            })
        });
    });
}

fn contended_concurrent(c: &mut Criterion) {
    let rt = platoon::Runtime::new().unwrap();
    let s = Rc::new(Semaphore::new(5));

    c.bench_function("contended_concurrent", |b| {
        b.iter(|| {
            let s = s.clone();
            rt.block_on(async move {
                futures_util::join! {
                    task(s.clone()),
                    task(s.clone()),
                    task(s.clone()),
                    task(s.clone()),
                    task(s.clone()),
                    task(s.clone())
                };
            })
        });
    });
}

criterion::criterion_group!(
    semaphore,
    uncontended,
    uncontended_tokio,
    uncontended_concurrent,
    uncontended_concurrent_tokio,
    contended_concurrent,
    contended_concurrent_tokio,
);

criterion::criterion_main!(semaphore);
