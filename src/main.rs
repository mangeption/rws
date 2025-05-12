use std::io;
use tokio::net::{TcpListener, TcpStream};
use crate::handler::Handler;

mod frame;
mod handler;
mod handshake;
mod reader;
mod writer;


#[tokio::main]
async fn main() -> io::Result<()> {
    let listener = TcpListener::bind("127.0.0.1:8080").await?;
    loop {
        let (mut stream, _) = listener.accept().await?;

        tokio::spawn(async move {
            Handler::handle_connection(&mut stream).await;
        });
    }
}

// async fn handle_handshake() {}
