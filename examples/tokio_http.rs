use hyper::{server::conn::Http, service::service_fn, Body, Request, Response};
use std::{convert::Infallible, net::SocketAddr};
use tokio::net::TcpListener;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let addr: SocketAddr = ([127, 0, 0, 1], 8080).into();

    let tcp_listener = TcpListener::bind(addr).await?;
    loop {
        let (tcp_stream, _) = tcp_listener.accept().await?;
        tokio::task::spawn(async move {
            let _ = Http::new()
                .http1_only(true)
                .http1_keep_alive(true)
                .serve_connection(tcp_stream, service_fn(hello))
                .await;
        });
    }
}

async fn hello(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    Ok(Response::new(Body::from("Hello World!")))
}
