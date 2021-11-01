use std::net::SocketAddr;

use futures::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use smoller::net::{TcpListener, TcpStream};

fn main() {
    smoller::Runtime::new().unwrap().block_on(async {
        let listener = TcpListener::bind("127.0.0.1:3000".parse::<SocketAddr>().unwrap())
            .await
            .expect("failed to create TCP listener");

        loop {
            let stream = listener.accept().await.expect("client connection failed");
            smoller::spawn(handle_connection(stream));
        }
    });
}

async fn handle_connection(mut stream: TcpStream) {
    // === READ RAW BYTES ===
    let mut request = Vec::new();
    let mut reader = BufReader::new(&mut stream);
    reader
        .read_until(b'\n', &mut request)
        .await
        .expect("failed to read from stream");

    // === WRITE RESPONSE ===
    let response = concat!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n",
        "Hello world!"
    );
    stream
        .write(response.as_bytes())
        .await
        .expect("failed to write to stream");
    stream.flush().await.expect("failed to flush stream");
}
