use hyper::{server::conn::Http, service::service_fn, Body, Request, Response};
use smoller::net::TcpListener;
use std::{convert::Infallible, net::SocketAddr};

fn main() {
    smoller::block_on(async move {
        let addr: SocketAddr = ([127, 0, 0, 1], 8080).into();

        let tcp_listener = TcpListener::bind(addr).await.unwrap();
        loop {
            let tcp_stream = tcp_listener.accept().await.unwrap();

            smoller::task::spawn(async move {
                let _ = Http::new()
                    .http1_only(true)
                    .http1_keep_alive(true)
                    .serve_connection(compat::HyperStream(tcp_stream), service_fn(hello))
                    .await;
            });
        }
    })
    .unwrap();
}

async fn hello(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    Ok(Response::new(Body::from("Hello World!")))
}

pub mod compat {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use futures::{AsyncRead, AsyncWrite};
    use smoller::net::TcpStream;
    use tokio::io::ReadBuf;

    pub struct HyperStream(pub TcpStream);

    impl tokio::io::AsyncRead for HyperStream {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let unfilled = buf.initialize_unfilled();
            let poll = Pin::new(&mut self.0).poll_read(cx, unfilled);
            if let Poll::Ready(Ok(num)) = &poll {
                buf.advance(*num);
            }
            poll.map_ok(|_| ())
        }
    }

    impl tokio::io::AsyncWrite for HyperStream {
        fn poll_write(
            mut self: Pin<&mut Self>,
            cx: &mut Context,
            buf: &[u8],
        ) -> Poll<std::io::Result<usize>> {
            Pin::new(&mut self.0).poll_write(cx, buf)
        }

        fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.0).poll_flush(cx)
        }

        fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<std::io::Result<()>> {
            Pin::new(&mut self.0).poll_close(cx)
        }
    }
}
