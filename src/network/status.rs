use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub const DEFAULT_PORT: u16 = 25565;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(3000);

const LEGACY_PROTOCOL_VERSION: u8 = 74;
const MAX_PACKET_LENGTH: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerSample {
    pub name: String,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServerStatus {
    pub error: bool,
    pub ms: u64,
    pub version: String,
    #[serde(rename = "playersConnect")]
    pub players_connect: u64,
    #[serde(rename = "playersMax")]
    pub players_max: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favicon: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub players: Vec<PlayerSample>,
    pub legacy: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Socket timed out when connecting to {0}")]
    Timeout(String),
    #[error("invalid response from {address}: {reason}")]
    Invalid { address: String, reason: String },
    #[error("invalid status json: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Probe {
    V16,
    V14,
    Beta,
}

#[derive(Debug, Clone)]
pub struct Status {
    host: String,
    port: u16,
    timeout: Duration,
}

impl Status {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            timeout: DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub async fn get_status(&self) -> Result<ServerStatus, Error> {
        let mut last = match self.modern().await {
            Ok(status) => return Ok(status),
            Err(error) => error,
        };
        for probe in [Probe::V16, Probe::V14, Probe::Beta] {
            match self.legacy(probe).await {
                Ok(status) => return Ok(status),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    pub async fn modern(&self) -> Result<ServerStatus, Error> {
        let start = Instant::now();
        let mut stream = self.connect().await?;

        let mut handshake = Vec::new();
        write_varint(&mut handshake, 0);
        write_varint(&mut handshake, -1);
        write_string(&mut handshake, &self.host);
        handshake.extend_from_slice(&self.port.to_be_bytes());
        write_varint(&mut handshake, 1);
        let mut request = Vec::new();
        write_varint(&mut request, 0);
        let mut out = Vec::new();
        frame(&mut out, &handshake);
        frame(&mut out, &request);
        self.io(stream.write_all(&out)).await??;

        let packet = self.io(read_packet(&mut stream)).await??;
        let ms = start.elapsed().as_millis() as u64;
        let mut cursor = 0;
        let id =
            read_varint(&packet, &mut cursor).ok_or_else(|| self.invalid("truncated packet"))?;
        if id != 0 {
            return Err(self.invalid(&format!("unexpected packet id {id}")));
        }
        let json =
            read_string(&packet, &mut cursor).ok_or_else(|| self.invalid("truncated json"))?;
        parse_modern(&json, ms).map_err(Error::Json)
    }

    async fn legacy(&self, probe: Probe) -> Result<ServerStatus, Error> {
        let start = Instant::now();
        let mut stream = self.connect().await?;
        let payload = match probe {
            Probe::V16 => legacy_16_probe(&self.host, self.port),
            Probe::V14 => vec![0xFE, 0x01],
            Probe::Beta => vec![0xFE],
        };
        self.io(stream.write_all(&payload)).await??;

        let first = self.io(stream.read_u8()).await??;
        if first != 0xFF {
            return Err(self.invalid(&format!("unexpected legacy packet 0x{first:02X}")));
        }
        let length = self.io(stream.read_u16()).await?? as usize;
        let mut bytes = vec![0u8; length * 2];
        self.io(stream.read_exact(&mut bytes)).await??;
        let ms = start.elapsed().as_millis() as u64;
        let text = decode_utf16be(&bytes);
        parse_legacy(&text, ms).ok_or_else(|| self.invalid("unrecognised legacy response"))
    }

    async fn connect(&self) -> Result<TcpStream, Error> {
        let stream = self
            .io(TcpStream::connect((self.host.as_str(), self.port)))
            .await??;
        Ok(stream)
    }

    async fn io<T>(&self, future: impl std::future::Future<Output = T>) -> Result<T, Error> {
        tokio::time::timeout(self.timeout, future)
            .await
            .map_err(|_| Error::Timeout(self.address()))
    }

    fn invalid(&self, reason: &str) -> Error {
        Error::Invalid {
            address: self.address(),
            reason: reason.to_owned(),
        }
    }
}

pub fn parse_modern(json: &str, ms: u64) -> Result<ServerStatus, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let players = value.get("players");
    Ok(ServerStatus {
        error: false,
        ms,
        version: value["version"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        players_connect: players
            .and_then(|p| p.get("online"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        players_max: players
            .and_then(|p| p.get("max"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        protocol: value["version"]["protocol"].as_i64(),
        description: value.get("description").map(text_of),
        favicon: value
            .get("favicon")
            .and_then(|f| f.as_str())
            .map(str::to_owned),
        players: players
            .and_then(|p| p.get("sample"))
            .and_then(|s| serde_json::from_value(s.clone()).ok())
            .unwrap_or_default(),
        legacy: false,
    })
}

pub fn parse_legacy(text: &str, ms: u64) -> Option<ServerStatus> {
    if let Some(rest) = text.strip_prefix("§1\0") {
        let parts: Vec<&str> = rest.split('\0').collect();
        if parts.len() < 5 {
            return None;
        }
        return Some(ServerStatus {
            error: false,
            ms,
            version: parts[1].to_owned(),
            players_connect: parts[3].parse().ok()?,
            players_max: parts[4].parse().ok()?,
            protocol: parts[0].parse().ok(),
            description: Some(parts[2].to_owned()),
            favicon: None,
            players: Vec::new(),
            legacy: true,
        });
    }
    let parts: Vec<&str> = text.rsplitn(3, '§').collect();
    if parts.len() != 3 {
        return None;
    }
    Some(ServerStatus {
        error: false,
        ms,
        version: String::new(),
        players_connect: parts[1].parse().ok()?,
        players_max: parts[0].parse().ok()?,
        protocol: None,
        description: Some(parts[2].to_owned()),
        favicon: None,
        players: Vec::new(),
        legacy: true,
    })
}

pub fn text_of(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(items) => items.iter().map(text_of).collect(),
        serde_json::Value::Object(object) => {
            let mut text = object
                .get("text")
                .or_else(|| object.get("translate"))
                .and_then(|t| t.as_str())
                .unwrap_or_default()
                .to_owned();
            if let Some(extra) = object.get("extra") {
                text.push_str(&text_of(extra));
            }
            text
        }
        _ => String::new(),
    }
}

fn legacy_16_probe(host: &str, port: u16) -> Vec<u8> {
    let channel = encode_utf16be("MC|PingHost");
    let host_bytes = encode_utf16be(host);
    let mut payload = vec![0xFE, 0x01, 0xFA];
    payload.extend_from_slice(&((channel.len() / 2) as u16).to_be_bytes());
    payload.extend_from_slice(&channel);
    payload.extend_from_slice(&((7 + host_bytes.len()) as u16).to_be_bytes());
    payload.push(LEGACY_PROTOCOL_VERSION);
    payload.extend_from_slice(&((host_bytes.len() / 2) as u16).to_be_bytes());
    payload.extend_from_slice(&host_bytes);
    payload.extend_from_slice(&(port as i32).to_be_bytes());
    payload
}

async fn read_packet(stream: &mut TcpStream) -> std::io::Result<Vec<u8>> {
    let mut length = 0usize;
    for index in 0..5 {
        let byte = stream.read_u8().await?;
        length |= ((byte & 0x7F) as usize) << (7 * index);
        if byte & 0x80 == 0 {
            break;
        }
    }
    if length == 0 || length > MAX_PACKET_LENGTH {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid packet length {length}"),
        ));
    }
    let mut packet = vec![0u8; length];
    stream.read_exact(&mut packet).await?;
    Ok(packet)
}

pub fn write_varint(buffer: &mut Vec<u8>, value: i32) {
    let mut value = value as u32;
    loop {
        if value & !0x7F == 0 {
            buffer.push(value as u8);
            return;
        }
        buffer.push((value as u8 & 0x7F) | 0x80);
        value >>= 7;
    }
}

pub fn read_varint(buffer: &[u8], cursor: &mut usize) -> Option<i32> {
    let mut value = 0u32;
    for index in 0..5 {
        let byte = *buffer.get(*cursor)?;
        *cursor += 1;
        value |= ((byte & 0x7F) as u32) << (7 * index);
        if byte & 0x80 == 0 {
            return Some(value as i32);
        }
    }
    None
}

fn write_string(buffer: &mut Vec<u8>, value: &str) {
    write_varint(buffer, value.len() as i32);
    buffer.extend_from_slice(value.as_bytes());
}

fn read_string(buffer: &[u8], cursor: &mut usize) -> Option<String> {
    let length = read_varint(buffer, cursor)? as usize;
    let end = cursor.checked_add(length)?;
    let bytes = buffer.get(*cursor..end)?;
    *cursor = end;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn frame(out: &mut Vec<u8>, packet: &[u8]) {
    write_varint(out, packet.len() as i32);
    out.extend_from_slice(packet);
}

fn decode_utf16be(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn encode_utf16be(text: &str) -> Vec<u8> {
    text.encode_utf16()
        .flat_map(|unit| unit.to_be_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[test]
    fn varint_roundtrip() {
        for value in [0, 1, 127, 128, 255, 300, 2_147_483_647, -1, -2_147_483_648] {
            let mut buffer = Vec::new();
            write_varint(&mut buffer, value);
            let mut cursor = 0;
            assert_eq!(read_varint(&buffer, &mut cursor), Some(value));
            assert_eq!(cursor, buffer.len());
        }
        let mut negative = Vec::new();
        write_varint(&mut negative, -1);
        assert_eq!(negative, vec![0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }

    #[test]
    fn parses_modern_json_with_text_components() {
        let json = r#"{"version":{"name":"1.21.1","protocol":767},"players":{"max":100,"online":2,"sample":[{"name":"Luuxis","id":"f07f"}]},"description":{"text":"Hello ","extra":[{"text":"world","color":"red"}]},"favicon":"data:image/png;base64,AAAA","enforcesSecureChat":true}"#;
        let status = parse_modern(json, 12).unwrap();
        assert_eq!(status.version, "1.21.1");
        assert_eq!(status.protocol, Some(767));
        assert_eq!(status.players_connect, 2);
        assert_eq!(status.players_max, 100);
        assert_eq!(status.description.as_deref(), Some("Hello world"));
        assert_eq!(status.players[0].name, "Luuxis");
        assert!(!status.legacy);
        assert!(!status.error);
        let plain = parse_modern(r#"{"version":{"name":"1.8.9","protocol":47},"players":{"max":20,"online":0},"description":"A Minecraft Server"}"#, 1).unwrap();
        assert_eq!(plain.description.as_deref(), Some("A Minecraft Server"));
        let serialised = serde_json::to_value(&plain).unwrap();
        assert_eq!(serialised["playersMax"], 20);
        assert_eq!(serialised["playersConnect"], 0);
    }

    #[test]
    fn parses_legacy_responses() {
        let status =
            parse_legacy("§1\u{0}78\u{0}1.6.4\u{0}A Minecraft Server\u{0}3\u{0}20", 5).unwrap();
        assert_eq!(status.protocol, Some(78));
        assert_eq!(status.version, "1.6.4");
        assert_eq!(status.description.as_deref(), Some("A Minecraft Server"));
        assert_eq!(status.players_connect, 3);
        assert_eq!(status.players_max, 20);
        assert!(status.legacy);

        let beta = parse_legacy("A Beta Server§3§20", 5).unwrap();
        assert_eq!(beta.version, "");
        assert_eq!(beta.description.as_deref(), Some("A Beta Server"));
        assert_eq!(beta.players_connect, 3);
        assert_eq!(beta.players_max, 20);
        assert!(parse_legacy("garbage", 1).is_none());
    }

    #[test]
    fn builds_legacy_16_probe() {
        let probe = legacy_16_probe("mc", 25565);
        assert_eq!(&probe[..3], &[0xFE, 0x01, 0xFA]);
        assert_eq!(&probe[3..5], &[0x00, 0x0B]);
        assert_eq!(&probe[5..27], &encode_utf16be("MC|PingHost")[..]);
        assert_eq!(&probe[27..29], &(7u16 + 4).to_be_bytes());
        assert_eq!(probe[29], LEGACY_PROTOCOL_VERSION);
        assert_eq!(&probe[30..32], &[0x00, 0x02]);
        assert_eq!(&probe[32..36], &encode_utf16be("mc")[..]);
        assert_eq!(&probe[36..40], &25565i32.to_be_bytes());
    }

    #[tokio::test]
    async fn pings_a_modern_mock_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = vec![0u8; 64];
            let _ = socket.read(&mut buffer).await.unwrap();
            let mut payload = Vec::new();
            write_varint(&mut payload, 0);
            write_string(
                &mut payload,
                r#"{"version":{"name":"1.20.1","protocol":763},"players":{"max":50,"online":7},"description":{"text":"Mock"}}"#,
            );
            let mut out = Vec::new();
            frame(&mut out, &payload);
            socket.write_all(&out).await.unwrap();
        });
        let status = Status::new("127.0.0.1", port).get_status().await.unwrap();
        assert_eq!(status.version, "1.20.1");
        assert_eq!(status.players_connect, 7);
        assert_eq!(status.players_max, 50);
        assert_eq!(status.description.as_deref(), Some("Mock"));
        assert!(!status.legacy);
    }

    #[tokio::test]
    async fn falls_back_to_legacy_mock_server() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut first = [0u8; 1];
                if socket.read_exact(&mut first).await.is_err() {
                    continue;
                }
                if first[0] != 0xFE {
                    drop(socket);
                    continue;
                }
                let text = encode_utf16be("§1\u{0}74\u{0}1.6.2\u{0}Legacy\u{0}1\u{0}8");
                let mut out = vec![0xFF];
                out.extend_from_slice(&((text.len() / 2) as u16).to_be_bytes());
                out.extend_from_slice(&text);
                socket.write_all(&out).await.unwrap();
            }
        });
        let status = Status::new("127.0.0.1", port)
            .with_timeout(Duration::from_millis(1500))
            .get_status()
            .await
            .unwrap();
        assert!(status.legacy);
        assert_eq!(status.version, "1.6.2");
        assert_eq!(status.protocol, Some(74));
        assert_eq!(status.description.as_deref(), Some("Legacy"));
        assert_eq!(status.players_connect, 1);
        assert_eq!(status.players_max, 8);
    }
}
