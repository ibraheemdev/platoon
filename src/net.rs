use std::io::{self, IoSlice, IoSliceMut, Read, Write};
use std::net::SocketAddr;
use std::os::unix::prelude::AsRawFd;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::core::Direction;
use crate::{util, Runtime};

pub struct TcpListener {
    rt: Runtime,
    sys: std::net::TcpListener,
    id: usize,
}

impl TcpListener {
    pub async fn bind<A: Into<SocketAddr>>(addr: A) -> io::Result<TcpListener> {
        let addr = addr.into();
        let sys = std::net::TcpListener::bind(addr)?;
        sys.set_nonblocking(true)?;

        let rt = Runtime::current().unwrap();

        let id = rt.core.insert_source(sys.as_raw_fd())?;

        Ok(TcpListener { id, sys, rt })
    }

    pub fn poll_accept(&self, cx: &mut Context) -> Poll<io::Result<TcpStream>> {
        loop {
            match self.rt.core.poll_ready(self.id, Direction::Read, cx) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => return Poll::Pending,
            }

            match self.sys.accept() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Poll::Ready(Err(err)),
                Ok((sys, _)) => {
                    sys.set_nonblocking(true)?;
                    let id = self.rt.core.insert_source(sys.as_raw_fd())?;
                    return Poll::Ready(Ok(TcpStream {
                        id,
                        rt: self.rt.clone(),
                        sys,
                    }));
                }
            }
        }
    }

    pub async fn accept(&self) -> io::Result<TcpStream> {
        loop {
            match self.sys.accept() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err),
                Ok((sys, _)) => {
                    sys.set_nonblocking(true)?;
                    let id = self.rt.core.insert_source(sys.as_raw_fd())?;
                    return Ok(TcpStream {
                        id,
                        rt: self.rt.clone(),
                        sys,
                    });
                }
            }

            util::poll_fn(|cx| self.rt.core.poll_ready(self.id, Direction::Read, cx)).await?;
        }
    }
}

pub struct TcpStream {
    rt: Runtime,
    sys: std::net::TcpStream,
    id: usize,
}

impl TcpStream {
    pub fn from_std(sys: std::net::TcpStream) -> io::Result<TcpStream> {
        let rt = Runtime::current().unwrap();
        let id = rt.core.insert_source(sys.as_raw_fd())?;
        Ok(Self { rt, id, sys })
    }

    pub fn into_std(self) -> io::Result<std::net::TcpStream> {
        self.rt.core.remove_source(self.id)?;
        Ok(self.sys)
    }
}

impl futures::AsyncRead for TcpStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.sys.read(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            match self.rt.core.poll_ready(self.id, Direction::Read, cx) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_read_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.sys.read_vectored(bufs) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            match self.rt.core.poll_ready(self.id, Direction::Read, cx) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl futures::AsyncWrite for TcpStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.sys.write(buf) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            match self.rt.core.poll_ready(self.id, Direction::Write, cx) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        loop {
            match self.sys.write_vectored(bufs) {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            match self.rt.core.poll_ready(self.id, Direction::Write, cx) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        loop {
            match self.sys.flush() {
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                res => return Poll::Ready(res),
            }

            match self.rt.core.poll_ready(self.id, Direction::Write, cx) {
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(_)) => {}
                Poll::Pending => return Poll::Pending,
            }
        }
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}
