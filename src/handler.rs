use crate::frame::Frame;
use crate::frame::Opcode;
use crate::handshake::do_handshake;
use crate::reader::Reader;
use crate::writer::Writer;
use tokio::io::{BufReader, BufWriter};
use tokio::net::TcpStream;

pub struct Handler {}

impl Handler {
    pub async fn handle_connection(stream: &mut TcpStream) {
        let (read_half, write_half) = stream.split();
        let mut read_half = BufReader::new(read_half);
        let mut write_half = BufWriter::new(write_half);

        match do_handshake(&mut read_half, &mut write_half).await {
            Ok(_) => {
                println!("Handshake successful");
            }
            Err(e) => {
                println!("Handshake failed: {}", e);
                return;
            }
        }

        let mut reader = Reader::new(64 * 1024 * 1024);

        loop {
            let frame = match reader.read(&mut read_half).await {
                Ok(frame) => frame,
                Err(e) => {
                    break;
                }
            };

            match frame.opcode {
                Opcode::Text => {
                    if let Err(e) = Writer::write_frame(&frame, &mut write_half).await {
                        break;
                    }
                }
                Opcode::Close => {
                    if let Ok(reply) = Frame::new_close_reply(frame.data) {
                        let _ = Writer::write_frame(&reply, &mut write_half).await;
                    }
                    break;
                }
                Opcode::Ping => {
                    let pong_frame = Frame::new(Opcode::Pong, frame.data);
                    if let Err(e) = Writer::write_frame(&pong_frame, &mut write_half).await {
                        break;
                    }
                }
                Opcode::Pong => {}
                Opcode::Binary => {
                    if let Err(e) = Writer::write_frame(&frame, &mut write_half).await {
                        break;
                    }
                }
                Opcode::Continuation => {
                    if let Err(e) = Writer::write_frame(&frame, &mut write_half).await {
                        break;
                    }
                }
            }
        }
    }
}
