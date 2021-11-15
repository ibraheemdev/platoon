use static_assertions::assert_not_impl_any;

#[test]
fn assertions() {
    assert_not_impl_any!(platoon::Runtime: Send, Sync);
    assert_not_impl_any!(platoon::net::TcpListener: Send, Sync);
    assert_not_impl_any!(platoon::net::TcpStream: Send, Sync);
    assert_not_impl_any!(platoon::time::Interval: Send, Sync);
    assert_not_impl_any!(platoon::time::Sleep: Send, Sync);
    assert_not_impl_any!(platoon::time::Timeout<()>: Send, Sync);
    assert_not_impl_any!(platoon::task::JoinHandle<()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::Semaphore: Send, Sync);
    assert_not_impl_any!(platoon::sync::Mutex<()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::MutexGuard<'_, ()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::RwLock<()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::RwLockReadGuard<()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::RwLockWriteGuard<()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::oneshot::Sender<()>: Send, Sync);
    assert_not_impl_any!(platoon::sync::oneshot::Receiver<()>: Send, Sync);
}
