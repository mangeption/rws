use std::io;
use thiserror::Error;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Opcode {
    Continuation = 0x0, // 0000
    Text = 0x1,         // 0001
    Binary = 0x2,       // 0010
    Close = 0x8,        // 1000
    Ping = 0x9,         // 1001
    Pong = 0xa,         // 1010
}

impl TryFrom<u8> for Opcode {
    type Error = FrameError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xa => Opcode::Pong,
            _ => return Err(FrameError::InvalidOpCode(value)),
        })
    }
}

impl Opcode {
    pub fn is_control(&self) -> bool {
        matches!(self, Opcode::Close | Opcode::Ping | Opcode::Pong)
    }
}

#[derive(Debug)]
pub struct Frame {
    pub fin: bool,
    pub opcode: Opcode,
    pub len: usize,
    pub data: Vec<u8>,
}

#[derive(Error, Debug)]
pub enum FrameError {
    #[error("Invalid UTF-8")]
    InvalidUTF8,
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Invalid OpCode: {0}")]
    InvalidOpCode(u8),
    #[error("Invalid continuation: {0}")]
    InvalidContinuation(u8),
    #[error("Invalid control fin: {0}")]
    InvalidControlFin(u8),
    #[error("Invalid payload length: {0}")]
    InvalidPayloadLength(u64),
    #[error("Frame too large")]
    FrameTooLarge,
    #[error("Ping Frame too large")]
    PingFrameTooLarge,
    #[error("Reserved bits are not zero")]
    ReservedBitsNotZero,
    #[error("Invalid fragment")]
    InvalidFragment,
    #[error("Invalid close frame")]
    InvalidCloseFrame,
}

pub enum CloseCode {
    Normal,
    Away,
    Protocol,
    Unsupported,
    Reserved(u16),
    Status,
    Abnormal,
    Invalid,
    Policy,
    Size,
    Extension,
    Error,
    Restart,
    Again,
    Tls,
    Bad(u16),
    Library(u16),
}

impl CloseCode {
    pub fn is_allowed(self) -> bool {
        !matches!(self, CloseCode::Bad(_) | CloseCode::Reserved(_) | CloseCode::Status | CloseCode::Abnormal | CloseCode::Tls)
      }
}

impl From<u16> for CloseCode {
    fn from(code: u16) -> CloseCode {
        match code {
            1000 => CloseCode::Normal,
            1001 => CloseCode::Away,
            1002 => CloseCode::Protocol,
            1003 => CloseCode::Unsupported,
            1005 => CloseCode::Status,
            1006 => CloseCode::Abnormal,
            1007 => CloseCode::Invalid,
            1008 => CloseCode::Policy,
            1009 => CloseCode::Size,
            1010 => CloseCode::Extension,
            1011 => CloseCode::Error,
            1012 => CloseCode::Restart,
            1013 => CloseCode::Again,
            1015 => CloseCode::Tls,
            1..=999 => CloseCode::Bad(code),
            // Library codes (3000-3999)
            // Reserved codes (1016-2999)
            1016..=2999 => CloseCode::Reserved(code),
            3000..=4999 => CloseCode::Library(code),
            // Any other code is considered reserved
            _ => CloseCode::Bad(code),
        }
    }
}

impl From<CloseCode> for u16 {
    fn from(code: CloseCode) -> u16 {
        match code {
            CloseCode::Normal => 1000,
            CloseCode::Away => 1001,
            CloseCode::Protocol => 1002,
            CloseCode::Unsupported => 1003,
            CloseCode::Status => 1005,
            CloseCode::Abnormal => 1006,
            CloseCode::Invalid => 1007,
            CloseCode::Policy => 1008,
            CloseCode::Size => 1009,
            CloseCode::Extension => 1010,
            CloseCode::Error => 1011,
            CloseCode::Restart => 1012,
            CloseCode::Again => 1013,
            CloseCode::Tls => 1015,
            CloseCode::Reserved(code) => code,
            CloseCode::Library(code) => code,
            CloseCode::Bad(code) => code,
        }
    }
}

impl Frame {
    pub fn new(opcode: Opcode, data: Vec<u8>) -> Self {
        Self {
            fin: true,
            opcode,
            len: data.len(),
            data,
        }
    }

    pub fn new_close_reply(data: Vec<u8>) -> Result<Self, FrameError> {
        match data.len() {
            0 => Ok(Self::new(Opcode::Close, data)),
            1 => Err(FrameError::InvalidCloseFrame),
            _ => {
                // First two bytes must be a valid close code
                let code = CloseCode::from(u16::from_be_bytes([data[0], data[1]]));
                if !code.is_allowed() {
                    return Ok(Self::close(1002, &data[2..]));
                }

                // If there's more data, it must be valid UTF-8
                if data.len() > 2 && !simdutf8::basic::from_utf8(&data[2..]).is_ok() {
                    return Err(FrameError::InvalidUTF8);
                }

                Ok(Self::new(Opcode::Close, data))
            }
        }
    }

    pub fn close(code: u16, reason: &[u8]) -> Self {
        let mut payload = Vec::with_capacity(2 + reason.len());
        payload.extend_from_slice(&code.to_be_bytes());
        payload.extend_from_slice(reason);

        return Self { fin: true, opcode: Opcode::Close, len: payload.len(), data: payload }
    }
}
