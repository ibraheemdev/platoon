use std::io;
use std::task::Waker;
use std::time::Duration;

pub trait Park: 'static {
    fn park(&self, wakers: &mut Vec<Waker>) -> io::Result<()>;
    fn park_timeout(&self, duration: Duration, wakers: &mut Vec<Waker>) -> io::Result<()>;
}
