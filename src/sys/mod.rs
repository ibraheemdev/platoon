#[cfg(any(target_os = "linux", target_os = "android"))]
#[path = "./epoll.rs"]
mod sys;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
))]
#[path = "./kqueue.rs"]
mod sys;

pub use sys::{Poller, SysEvent};

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

pub trait AsRaw {
    fn as_raw(&self) -> Raw;
}

pub use raw::Raw;

#[cfg(unix)]
mod raw {
    use std::os::unix::io::{AsRawFd, RawFd};

    impl<T> super::AsRaw for T
    where
        T: AsRawFd,
    {
        fn as_raw(&self) -> Raw {
            self.as_raw_fd()
        }
    }

    pub type Raw = RawFd;
}

#[cfg(windows)]
mod raw {
    use std::os::windows::io::{AsRawSocket, RawSocket};

    impl<T> super::AsRaw for T
    where
        T: AsRawSocket,
    {
        fn as_raw(&self) -> Raw {
            self.as_raw_socket()
        }
    }

    pub type Raw = RawSocket;
}
