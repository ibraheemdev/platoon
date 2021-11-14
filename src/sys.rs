pub use poller::{Poller, SysEvent};
pub use raw::Raw;

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

#[derive(Default)]
pub struct Event {
    pub key: usize,
    pub readable: bool,
    pub writable: bool,
}

pub trait AsRaw {
    fn as_raw(&self) -> Raw;
}

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

pub fn connect(
    addr: SockAddr,
    domain: Domain,
    protocol: Option<Protocol>,
) -> std::io::Result<Socket> {
    let sock_type = Type::STREAM;
    #[cfg(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd"
    ))]
    let sock_type = sock_type.nonblocking();

    let socket = Socket::new(domain, sock_type, protocol)?;

    #[cfg(not(any(
        target_os = "android",
        target_os = "dragonfly",
        target_os = "freebsd",
        target_os = "fuchsia",
        target_os = "illumos",
        target_os = "linux",
        target_os = "netbsd",
        target_os = "openbsd"
    )))]
    socket.set_nonblocking(true)?;

    match socket.connect(&addr) {
        Ok(_) => {}
        #[cfg(unix)]
        Err(err) if err.raw_os_error() == Some(libc::EINPROGRESS) => {}
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
        Err(err) => return Err(err),
    }

    Ok(socket)
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "dragonfly",
))]
mod poller {
    use super::Event;
    use crate::util::syscall;

    use libc::{
        ENOENT, EPIPE, EVFILT_READ, EVFILT_WRITE, EV_ADD, EV_DELETE, EV_EOF, EV_ERROR, EV_ONESHOT,
        EV_RECEIPT, FD_CLOEXEC, F_SETFD,
    };
    use std::io;
    use std::os::unix::io::RawFd;
    use std::time::Duration;

    pub struct Poller {
        fd: RawFd,
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            syscall!(kqueue())
                .and_then(|fd| syscall!(fcntl(fd, F_SETFD, FD_CLOEXEC)).map(|_| fd))
                .map(|fd| Self { fd })
        }

        pub fn poll(
            &self,
            events: &mut Vec<SysEvent>,
            timeout: Option<Duration>,
        ) -> io::Result<usize> {
            let timeout = timeout.map(|t| libc::timespec {
                tv_sec: t.as_secs() as libc::time_t,
                tv_nsec: t.subsec_nanos() as libc::c_long,
            });

            events.clear();
            syscall!(kevent(
                self.fd,
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.capacity() as _,
                timeout
                    .map(|s| &s as *const _)
                    .unwrap_or(std::ptr::null_mut())
            ))
            .map(|n| {
                unsafe { events.set_len(n as _) };
                n as _
            })
        }

        pub fn add(&self, fd: RawFd, event: Event) -> io::Result<()> {
            self.update(fd, event)
        }

        pub fn delete(&self, fd: RawFd) -> io::Result<()> {
            self.update(fd, Event::default())
        }

        pub fn update(&self, fd: RawFd, event: Event) -> io::Result<()> {
            let mut changes = [
                kchange(fd, event.readable, EVFILT_READ, event.key),
                kchange(fd, event.writable, EVFILT_WRITE, event.key),
            ];

            syscall!(kevent(
                self.fd,
                changes.as_ptr(),
                changes.len() as _,
                changes.as_mut_ptr(),
                changes.len() as _,
                std::ptr::null(),
            ))?;

            for event in &changes {
                if (event.flags & EV_ERROR) != 0
                && event.data != 0
                // the event we tried to modify wasn't found, which is fine
                && event.data != ENOENT as _
                // https://github.com/tokio-rs/mio/issues/582
                && event.data != EPIPE as _
                {
                    return Err(io::Error::from_raw_os_error(event.data as _));
                }
            }

            Ok(())
        }
    }

    impl Drop for Poller {
        fn drop(&mut self) {
            let _ = syscall!(close(self.fd));
        }
    }

    impl From<&SysEvent> for Event {
        fn from(sys: &SysEvent) -> Self {
            Event {
                key: sys.udata as _,
                readable: sys.filter == EVFILT_READ,
                // https://github.com/golang/go/commit/23aad448b1e3f7c3b4ba2af90120bde91ac865b4
                writable: sys.filter == EVFILT_WRITE
                    || (sys.filter == EVFILT_READ && (sys.flags & EV_EOF) != 0),
            }
        }
    }

    fn kchange(fd: RawFd, interested: bool, filter: i16, key: usize) -> SysEvent {
        SysEvent {
            ident: fd as _,
            filter: filter as _,
            flags: EV_RECEIPT | interested.then(|| EV_ADD | EV_ONESHOT).unwrap_or(EV_DELETE),
            fflags: 0,
            data: 0,
            udata: key as _,
        }
    }

    pub type SysEvent = libc::kevent;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod poller {
    use super::Event;
    use crate::util::syscall;

    use libc::{
        c_int, EPOLLIN, EPOLLONESHOT, EPOLLOUT, EPOLLPRI, EPOLLRDHUP, EPOLL_CLOEXEC, EPOLL_CTL_ADD,
        EPOLL_CTL_DEL, EPOLL_CTL_MOD,
    };
    use std::io;
    use std::os::unix::io::RawFd;
    use std::time::Duration;

    pub struct Poller {
        fd: RawFd,
    }

    impl Poller {
        pub fn new() -> io::Result<Self> {
            syscall!(epoll_create1(EPOLL_CLOEXEC)).map(|fd| Self { fd })
        }

        pub fn add(&self, fd: RawFd, event: Event) -> io::Result<()> {
            let mut event = event.into();
            syscall!(epoll_ctl(self.fd, EPOLL_CTL_ADD, fd, &mut event as _)).map(drop)
        }

        pub fn update(&self, fd: RawFd, event: Event) -> io::Result<()> {
            let mut event = event.into();
            syscall!(epoll_ctl(self.fd, EPOLL_CTL_MOD, fd, &mut event as _)).map(drop)
        }

        pub fn delete(&self, fd: RawFd) -> io::Result<()> {
            syscall!(epoll_ctl(self.fd, EPOLL_CTL_DEL, fd, std::ptr::null_mut())).map(drop)
        }

        pub fn poll(
            &self,
            events: &mut Vec<SysEvent>,
            timeout: Option<Duration>,
        ) -> io::Result<usize> {
            let timeout = timeout.map(|to| to.as_millis() as c_int).unwrap_or(-1);

            syscall!(epoll_wait(
                self.fd,
                events.as_mut_ptr(),
                events.capacity() as _,
                timeout,
            ))
            .map(|n| unsafe {
                events.set_len(n as _);
                n as _
            })
        }
    }

    impl Drop for Poller {
        fn drop(&mut self) {
            let _ = syscall!(close(self.fd));
        }
    }

    impl From<&SysEvent> for Event {
        fn from(sys: &SysEvent) -> Self {
            Event {
                key: sys.u64 as _,
                readable: (sys.events as c_int & (EPOLLIN | EPOLLPRI)) != 0,
                writable: (sys.events as c_int & EPOLLOUT) != 0,
            }
        }
    }

    impl From<Event> for SysEvent {
        fn from(event: Event) -> Self {
            let mut flags = EPOLLONESHOT;
            if event.readable {
                flags = flags | EPOLLIN | EPOLLRDHUP;
            }
            if event.writable {
                flags |= EPOLLOUT;
            }
            SysEvent {
                events: flags as _,
                u64: event.key as _,
            }
        }
    }

    pub type SysEvent = libc::epoll_event;
}
