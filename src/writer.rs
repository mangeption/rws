use crate::frame::{Frame, FrameError};
use tokio::io::AsyncWriteExt;

pub struct Writer {}

impl Writer {
    pub async fn write_frame(
        frame: &Frame,
        writer: &mut (impl AsyncWriteExt + Unpin),
    ) -> Result<(), FrameError> {
        let mut first_byte = if frame.fin { 0b1000_0000 } else { 0b0000_0000 };

        first_byte |= frame.opcode as u8;
        writer.write_all(&[first_byte]).await?;

        if frame.len <= 125 {
            writer.write_all(&[frame.len as u8]).await?;
        } else if frame.len <= u16::MAX as usize {
            writer.write_all(&[126]).await?;
            writer.write_all(&(frame.len as u16).to_be_bytes()).await?;
        } else {
            writer.write_all(&[127]).await?;
            writer.write_all(&(frame.len as u64).to_be_bytes()).await?;
        };

        writer.write_all(&frame.data).await?;
        writer.flush().await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Opcode;

    #[tokio::test]
    async fn test_write_small_frame() {
        let mut buffer = Vec::new();
        let frame = Frame {
            fin: true,
            opcode: Opcode::Text,
            len: 5,
            data: b"Hello".to_vec(),
        };

        Writer::write_frame(&frame, &mut buffer).await.unwrap();
        assert_eq!(buffer[0], 0b1000_0001); // FIN + Text frame
        assert_eq!(buffer[1], 5); // Payload length
        assert_eq!(&buffer[2..], b"Hello");
    }

    #[tokio::test]
    async fn test_write_medium_frame() {
        let mut buffer = Vec::new();
        let data = vec![0; 256];
        let frame = Frame {
            fin: true,
            opcode: Opcode::Binary,
            len: 256,
            data,
        };

        Writer::write_frame(&frame, &mut buffer).await.unwrap();

        assert_eq!(buffer[0], 0b1000_0010); // FIN + Binary frame
        assert_eq!(buffer[1], 126); // Extended payload length indicator
        assert_eq!(u16::from_be_bytes([buffer[2], buffer[3]]), 256);
    }
}
