use std::fmt::Binary;

use crate::frame::{self, Frame, FrameError, Opcode};
use tokio::io::AsyncReadExt;

pub struct Reader {
    max_payload_size: usize,
    fragments: Fragments
}

pub struct Fragments {
    fragments: Option<Fragment>,
    op_code: Opcode,
}

pub enum Fragment {
    Text(Option<utf8::Incomplete>, Vec<u8>),
    Binary(Vec<u8>),
}

impl Fragment {
    fn take_buffer(self) -> Vec<u8> {
        match self {
            Fragment::Binary(buffer) => buffer,
            Fragment::Text(_, buffer) => buffer,
        }
    }
}

impl Fragments {
    pub fn new() -> Self {
        return Fragments {
            fragments: None,
            op_code: Opcode::Close,
        };
    }

    pub fn accumulate(&mut self, frame: Frame) -> Result<Option<Frame>, FrameError> {
        match frame.opcode {
            Opcode::Text | Opcode::Binary => {
                if frame.fin {
                    if self.fragments.is_some() {
                        return Err(FrameError::InvalidFragment);
                    }
                    if frame.opcode == Opcode::Text && !simdutf8::basic::from_utf8(&frame.data).is_ok() {
                        return Err(FrameError::InvalidUTF8);
                    }
                    return Ok(Some(frame));
                }

                self.fragments = match frame.opcode {
                    Opcode::Text => match utf8::decode(&frame.data) {
                        Ok(text) => Some(Fragment::Text(None, text.as_bytes().to_vec())),
                        Err(utf8::DecodeError::Incomplete {
                            valid_prefix,
                            incomplete_suffix,
                        }) => Some(Fragment::Text(
                            Some(incomplete_suffix),
                            valid_prefix.as_bytes().to_vec(),
                        )),
                        Err(utf8::DecodeError::Invalid { .. }) => {
                            return Err(FrameError::InvalidUTF8)
                        }
                    },
                    Opcode::Binary => Some(Fragment::Binary(frame.data)),
                    _ => unreachable!(),
                };
                self.op_code = frame.opcode;
            }
            Opcode::Continuation => match self.fragments.as_mut() {
                None => return Err(FrameError::InvalidContinuation(frame.opcode as u8)),
                Some(Fragment::Text(data, input)) => {
                    let mut tail = &frame.data[..];
                    if let Some(mut incomplete) = data.take() {
                        if let Some((result, rest)) = incomplete.try_complete(&frame.data) {
                            tail = rest;
                            match result {
                                Ok(text) => input.extend_from_slice(text.as_bytes()),
                                Err(_) => return Err(FrameError::InvalidUTF8),
                            }
                        } else {
                            tail = &[];
                            data.replace(incomplete);
                        }
                    }

                    match utf8::decode(tail) {
                        Ok(text) => {
                            input.extend_from_slice(text.as_bytes());
                        }
                        Err(utf8::DecodeError::Incomplete {
                            valid_prefix,
                            incomplete_suffix,
                        }) => {
                            input.extend_from_slice(valid_prefix.as_bytes());
                            data.replace(incomplete_suffix);
                        }
                        Err(utf8::DecodeError::Invalid { .. }) => {
                            return Err(FrameError::InvalidUTF8)
                        }
                    }

                    if frame.fin {
                        return Ok(Some(Frame::new(
                            self.op_code,
                            self.fragments.take().unwrap().take_buffer(),
                        )));
                    }
                }
                Some(Fragment::Binary(data)) => {
                    data.extend_from_slice(&frame.data);
                    if frame.fin {
                        return Ok(Some(Frame::new(
                            self.op_code,
                            self.fragments.take().unwrap().take_buffer(),
                        )));
                    }
                }
            },
            _ => return Ok(Some(frame)),
        }

        Ok(None)
    }
}

impl Reader {
    pub fn new(max_payload_size: usize) -> Self {
        Self { max_payload_size, fragments: Fragments::new() }
    }

    pub async fn read(
        &mut self,
        reader: &mut (impl AsyncReadExt + Unpin),
    ) -> Result<Frame, FrameError> {
        loop {
            let frame = self.read_frame(reader).await?;

            if let Some(res) = self.fragments.accumulate(frame)? {
                return Ok(res)
            }
        }
    }

    pub async fn read_frame(
        &self,
        reader: &mut (impl AsyncReadExt + Unpin),
    ) -> Result<Frame, FrameError> {

        let mut payload: Vec<u8> = vec![];
        let mut buf = [0; 2];
        reader.read_exact(&mut buf).await?;

        let fin = buf[0] & 0b1000_0000 != 0;
        let rsv1 = buf[0] & 0b0100_0000 != 0;
        let rsv2 = buf[0] & 0b0010_0000 != 0;
        let rsv3 = buf[0] & 0b0001_0000 != 0;
        if rsv1 || rsv2 || rsv3 {
            return Err(FrameError::ReservedBitsNotZero);
        }
        let opcode = Opcode::try_from(buf[0] & 0b0000_1111)?;

        if opcode.is_control() && !fin {
            return Err(FrameError::InvalidControlFin(opcode as u8));
        }

        // } else if opcode == Opcode::Continuation && is_first_frame {
        //     return Err(FrameError::InvalidContinuation(opcode as u8));
        // } else if opcode != Opcode::Continuation
        //     && !is_first_frame
        //     && !opcode.is_control()
        // {
        //     return Err(FrameError::InvalidContinuation(opcode as u8));
        // }

        let mask = buf[1] & 0b1000_0000 != 0;
        let payload_len = match buf[1] & 0b0111_1111 {
            126 => {
                let mut len_buf = [0; 2];
                reader.read_exact(&mut len_buf).await?;
                u16::from_be_bytes(len_buf) as u64
            }
            127 => {
                let mut len_buf = [0; 8];
                reader.read_exact(&mut len_buf).await?;
                u64::from_be_bytes(len_buf)
            }
            v => v as u64,
        };

        if opcode == Opcode::Ping && payload_len > 125 {
            return Err(FrameError::PingFrameTooLarge);
        }

        let mut cur_payload = vec![0; payload_len as usize];

        if mask {
            let mut mask_key = [0; 4];
            reader.read_exact(&mut mask_key).await?;
            reader.read_exact(&mut cur_payload).await?;
            for i in 0..cur_payload.len() {
                cur_payload[i] ^= mask_key[i % 4];
            }
        } else {
            reader.read_exact(&mut cur_payload).await?;
        }

        payload.extend(cur_payload);

        if opcode == Opcode::Close && payload.len() == 1{
            return  Err(FrameError::InvalidCloseFrame);
        }

        if payload.len() > self.max_payload_size {
            return Err(FrameError::FrameTooLarge);
        }

        Ok(Frame {
            fin: fin,
            opcode: opcode,
            len: payload.len(),
            data: payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_read_individual_frames() {
        // Create test data for:
        // 1. Text frame: "Hello"
        // 2. Pong frame: "pong"
        // 3. Continuation frame: " World" with fin=true
        let mut test_data = Vec::new();
        
        // Text frame: "Hello" (fin=false)
        test_data.extend_from_slice(&[
            0b0000_0001, // fin=0, rsv=0, opcode=1 (text)
            0b0000_0101, // mask=0, payload_len=5
        ]);
        test_data.extend_from_slice(b"Hello");

        // // Pong frame: "pong"
        // test_data.extend_from_slice(&[
        //     0b1000_1010, // fin=1, rsv=0, opcode=10 (pong)
        //     0b0000_0100, // mask=0, payload_len=4
        // ]);
        // test_data.extend_from_slice(b"pong");

        // // Continuation frame: " World" (fin=true)
        // test_data.extend_from_slice(&[
        //     0b1000_0000, // fin=1, rsv=0, opcode=0 (continuation)
        //     0b0000_0110, // mask=0, payload_len=6
        // ]);
        // test_data.extend_from_slice(b" World");

        let mut cursor = Cursor::new(test_data);
        let frame_reader = Reader::new(1024);

        // Read first frame (text)
        let frame = frame_reader.read_frame(&mut cursor).await.unwrap();
        assert_eq!(frame.opcode, Opcode::Text);
        assert_eq!(frame.data, b"Hello");
        assert!(!frame.fin);

        // // Read second frame (pong)
        // let frame = frame_reader.read_frame(&mut cursor).await.unwrap();
        // assert_eq!(frame.opcode, Opcode::Pong);
        // assert_eq!(frame.data, b"pong");
        // assert!(frame.fin);

        // // Read third frame (continuation)
        // let frame = frame_reader.read_frame(&mut cursor).await.unwrap();
        // assert_eq!(frame.opcode, Opcode::Continuation);
        // assert_eq!(frame.data, b" World");
        // assert!(frame.fin);
    }

    #[tokio::test]
    async fn test_read_complete_message() {
        // Create test data for a fragmented text message:
        // 1. Text frame: "Hello" (fin=false)
        // 2. Continuation frame: " World" (fin=true)
        let mut test_data = Vec::new();
        
        // Text frame: "Hello" (fin=false)
        test_data.extend_from_slice(&[
            0b0000_0001, // fin=0, rsv=0, opcode=1 (text)
            0b0000_0101, // mask=0, payload_len=5
        ]);
        test_data.extend_from_slice(b"Hello");

        // Pong frame: "pong"
        test_data.extend_from_slice(&[
            0b1000_1010, // fin=1, rsv=0, opcode=10 (pong)
            0b0000_0100, // mask=0, payload_len=4
        ]);
        test_data.extend_from_slice(b"pong");

        // Continuation frame: " World" (fin=true)
        test_data.extend_from_slice(&[
            0b1000_0000, // fin=1, rsv=0, opcode=0 (continuation)
            0b0000_0110, // mask=0, payload_len=6
        ]);
        test_data.extend_from_slice(b" World");

        let mut cursor = Cursor::new(test_data);
        let mut frame_reader = Reader::new(1024);

        // Read the pong message
        let frame = frame_reader.read(&mut cursor).await.unwrap();
        assert_eq!(frame.opcode, Opcode::Pong);
        assert_eq!(frame.data, b"pong");
        assert!(frame.fin);

        let frame = frame_reader.read(&mut cursor).await.unwrap();
        assert_eq!(frame.opcode, Opcode::Text);
        assert_eq!(frame.data, b"Hello World");
        assert!(frame.fin);
    }
}
