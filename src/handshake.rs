use base64::{engine::general_purpose::STANDARD, Engine as _};
use sha1::{Digest, Sha1};
use std::{collections::HashMap, io};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const WEBSOCKET_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
const REQUIRED_HEADERS: [&str; 3] = ["Sec-WebSocket-Key", "Upgrade", "Connection"];

#[derive(Error, Debug)]
pub enum HandshakeError {
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Missing required header: {0}")]
    MissingHeader(String),
    #[error("Invalid header value: {0}")]
    InvalidHeader(String),
}

pub async fn do_handshake(
    reader: &mut (impl AsyncBufReadExt + Unpin),
    writer: &mut (impl AsyncWriteExt + Unpin),
) -> Result<(), HandshakeError> {
    let headers = read_http_headers(reader).await?;
    validate_headers(&headers)?;
    send_response(writer, &headers).await?;
    Ok(())
}

async fn read_http_headers(
    reader: &mut (impl AsyncBufReadExt + Unpin),
) -> Result<HashMap<String, String>, HandshakeError> {
    let mut headers = HashMap::new();
    let mut request_line = String::new();

    reader.read_line(&mut request_line).await?;
    if !request_line.trim_end().starts_with("GET") {
        return Err(HandshakeError::InvalidHeader(
            "Must be GET request".to_string(),
        ));
    }

    loop {
        let mut line = String::new();
        let bytes_reads = reader.read_line(&mut line).await?;
        if bytes_reads == 0 || line.trim().is_empty() {
            break;
        }

        if let Some((key, value)) = line.split_once(":") {
            headers.insert(key.trim().to_string(), value.trim().to_string());
        } else {
            return Err(HandshakeError::InvalidHeader(
                "Invalid header format".to_string(),
            ));
        }
    }

    return Ok(headers);
}

fn validate_headers(headers: &HashMap<String, String>) -> Result<(), HandshakeError> {
    for header in REQUIRED_HEADERS {
        if !headers.contains_key(header) {
            return Err(HandshakeError::MissingHeader(header.to_string()));
        }
    }

    if headers.get("Upgrade").map(|v| v.to_lowercase()) != Some("websocket".to_string()) {
        return Err(HandshakeError::InvalidHeader(
            "Upgrade header must be websocket".to_string(),
        ));
    }

    if headers.get("Connection").map(|v| v.to_lowercase()) != Some("upgrade".to_string()) {
        return Err(HandshakeError::InvalidHeader(
            "Connection header must be upgrade".to_string(),
        ));
    }

    Ok(())
}

async fn send_response(
    writer: &mut (impl AsyncWriteExt + Unpin),
    headers: &HashMap<String, String>,
) -> Result<(), HandshakeError> {
    let response = generate_response(&headers["Sec-WebSocket-Key"]);
    writer.write_all(response.as_bytes()).await?;
    writer.flush().await?;
    Ok(())
}

fn generate_response(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(format!("{}{}", key, WEBSOCKET_GUID));
    let result = hasher.finalize();
    let accept_key = STANDARD.encode(result);
    format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufReader;
    use tokio::io::BufWriter;
    use tokio::net::TcpListener;
    use tokio::net::TcpStream;
    use tokio::time::timeout;

    struct MockStream {
        stream: TcpStream,
        _handle: tokio::task::JoinHandle<()>,
    }

    impl MockStream {
        async fn new(request: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();

            let handle = {
                let request = request.to_string();
                tokio::spawn(async move {
                    let mut stream = TcpStream::connect(addr).await.unwrap();
                    stream.write_all(request.as_bytes()).await.unwrap();
                })
            };

            let (stream, _) = timeout(Duration::from_secs(1), listener.accept())
                .await
                .unwrap()
                .unwrap();

            Self {
                stream,
                _handle: handle,
            }
        }
    }

    impl Drop for MockStream {
        fn drop(&mut self) {
            let _ = self.stream.shutdown();
            self._handle.abort();
        }
    }

    #[tokio::test]
    async fn test_read_http_headers() {
        let request = "GET / HTTP/1.1\r\n\
            Host: localhost:8080\r\n\
            Upgrade: websocket\r\n\
            Connection: Upgrade\r\n\
            Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let mut mock_stream = MockStream::new(request).await;

        let (reader, _) = mock_stream.stream.split();
        let mut reader = BufReader::new(reader);
        let headers = read_http_headers(&mut reader)
            .await
            .expect("Failed to read headers");
        assert_eq!(headers.len(), 4);
        assert_eq!(headers["Upgrade"], "websocket");
        assert_eq!(headers["Connection"], "Upgrade");
        assert_eq!(headers["Sec-WebSocket-Key"], "dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(headers["Host"], "localhost:8080");
    }

    #[tokio::test]
    async fn test_invalid_method() {
        let request = "POST / HTTP/1.1\r\n\
        Host: localhost:8080\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let mut mock_stream = MockStream::new(request).await;

        let (reader, _) = mock_stream.stream.split();
        let mut reader = BufReader::new(reader);
        let headers = read_http_headers(&mut reader).await;
        assert!(
            matches!(headers, Err(HandshakeError::InvalidHeader(s)) if s == "Must be GET request")
        );
    }

    #[tokio::test]
    async fn test_invalid_header_format() {
        let request = "GET / HTTP/1.1\r\n\
        Host\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let mut mock_stream = MockStream::new(request).await;

        let (reader, _) = mock_stream.stream.split();
        let mut reader = BufReader::new(reader);
        let headers = read_http_headers(&mut reader).await;
        assert!(
            matches!(headers, Err(HandshakeError::InvalidHeader(s)) if s == "Invalid header format")
        );
    }

    #[tokio::test]
    async fn test_validate_headers() {
        let headers = HashMap::from([
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Connection".to_string(), "Upgrade".to_string()),
            (
                "Sec-WebSocket-Key".to_string(),
                "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
            ),
        ]);
        let result = validate_headers(&headers);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_headers_missing_key() {
        let headers = HashMap::from([
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Connection".to_string(), "Upgrade".to_string()),
        ]);
        let result = validate_headers(&headers);
        assert!(
            matches!(result, Err(HandshakeError::MissingHeader(s)) if s == "Sec-WebSocket-Key")
        );
    }

    #[tokio::test]
    async fn test_validate_headers_invalid_upgrade() {
        let headers = HashMap::from([
            ("Upgrade".to_string(), "http".to_string()),
            ("Connection".to_string(), "Upgrade".to_string()),
            (
                "Sec-WebSocket-Key".to_string(),
                "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
            ),
        ]);
        let result = validate_headers(&headers);
        assert!(
            matches!(result, Err(HandshakeError::InvalidHeader(s)) if s == "Upgrade header must be websocket")
        );
    }

    #[tokio::test]
    async fn test_validate_headers_invalid_connection() {
        let headers = HashMap::from([
            ("Upgrade".to_string(), "websocket".to_string()),
            ("Connection".to_string(), "keep-alive".to_string()),
            (
                "Sec-WebSocket-Key".to_string(),
                "dGhlIHNhbXBsZSBub25jZQ==".to_string(),
            ),
        ]);
        let result = validate_headers(&headers);
        assert!(
            matches!(result, Err(HandshakeError::InvalidHeader(s)) if s == "Connection header must be upgrade")
        );
    }

    #[tokio::test]
    async fn test_generate_response() {
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let response = generate_response(key);
        assert!(response.starts_with("HTTP/1.1 101 Switching Protocols"));
        assert!(response.contains("Upgrade: websocket"));
        assert!(response.contains("Connection: Upgrade"));
        assert!(response.contains("Sec-WebSocket-Accept:"));
    }

    #[tokio::test]
    async fn test_do_handshake() {
        let request = "GET / HTTP/1.1\r\n\
        Host: localhost:8080\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\
        Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let mut mock_stream = MockStream::new(request).await;

        let (reader, writer) = mock_stream.stream.split();
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);
        let result = do_handshake(&mut reader, &mut writer).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_do_handshake_missing_key() {
        let request = "GET / HTTP/1.1\r\n\
        Host: localhost:8080\r\n\
        Upgrade: websocket\r\n\
        Connection: Upgrade\r\n\r\n";
        let mut mock_stream = MockStream::new(request).await;

        let (reader, writer) = mock_stream.stream.split();
        let mut reader = BufReader::new(reader);
        let mut writer = BufWriter::new(writer);
        let result = do_handshake(&mut reader, &mut writer).await;
        assert!(
            matches!(result, Err(HandshakeError::MissingHeader(s)) if s == "Sec-WebSocket-Key")
        );
    }
}
