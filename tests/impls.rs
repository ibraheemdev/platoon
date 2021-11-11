use static_assertions::assert_not_impl_any;

#[test]
fn assert_not_send() {
    assert_not_impl_any!(platoon::Runtime: Send, Sync);
    assert_not_impl_any!(platoon::net::TcpListener: Send, Sync);
    assert_not_impl_any!(platoon::net::TcpStream: Send, Sync);
    assert_not_impl_any!(platoon::time::Interval: Send, Sync);
    assert_not_impl_any!(platoon::time::Sleep: Send, Sync);
    assert_not_impl_any!(platoon::time::Timeout<()>: Send, Sync);
    assert_not_impl_any!(platoon::task::JoinHandle<()>: Send, Sync);
}
