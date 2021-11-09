use super::{syscall, Event};

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

    pub fn poll(&self, events: &mut Vec<SysEvent>, timeout: Option<Duration>) -> io::Result<usize> {
        let timeout = timeout.map(|to| to.as_millis() as c_int).unwrap_or(-1);

        syscall!(epoll_wait(
            self.fd,
            events.as_mut_ptr() as *mut SysEvent,
            events.capacity() as c_int,
            timeout as c_int,
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
            key: sys.u64 as usize,
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
            u64: event.key as u64,
        }
    }
}

pub type SysEvent = libc::epoll_event;
