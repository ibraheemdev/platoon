#[cfg(any(target_os = "linux", target_os = "android"))]
pub mod epoll;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub use epoll::{Poller, SysEvent};

macro_rules! syscall {
    ($fn:ident $args:tt) => {{
        let res = unsafe { libc::$fn $args };
        if res == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(res)
        }
    }};
}

pub(crate) use syscall;

#[derive(Default)]
pub struct Event {
    pub key: usize,
    pub readable: bool,
    pub writable: bool,
}
