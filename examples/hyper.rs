use std::convert::Infallible;
use std::net::SocketAddr;

use hyper::server::conn::Http;
use hyper::{service, Body, Request, Response};
use platoon::net::TcpListener;

fn main() {
    platoon::block_on(async move {
        let addr: SocketAddr = ([127, 0, 0, 1], 8080).into();

        let listener = TcpListener::bind(addr).await.unwrap();

        loop {
            let (stream, _) = listener.accept().await.unwrap();

            platoon::spawn(async move {
                let _ = Http::new()
                    .with_executor(compat::Spawn)
                    .serve_connection(compat::HyperStream(stream), service::service_fn(hello))
                    .await;
            });
        }
    });
}

async fn hello(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    Ok(Response::new(Body::from("Hello World!")))
}

mod compat {
    #[derive(Clone)]
    pub struct Spawn;

    impl<Fut> hyper::rt::Executor<Fut> for Spawn
    where
        Fut: Future + 'static,
    {
        fn execute(&self, fut: Fut) {
            platoon::spawn(fut);
        }
    }

    use std::pin::Pin;
    use std::task::{Context, Poll};

    use futures_io::{AsyncRead, AsyncWrite};
    use futures_util::Future;
    use platoon::net::TcpStream;
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
