pub mod nusb_backend;

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device_registry::{
    ConnectionKind, DeviceEdition, FallbackQuirkInfo, MatchedDevice, GAMESIR_VENDOR_ID,
};
use crate::protocol::state::RumbleState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureChannel {
    Xinput,
    Hid12,
}

/// Raw USB interrupt IN packet received from either Interface 0 (XInput) or Interface 1 (HID `0x12`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsbPacket {
    XInputIn { timestamp_us: u64, data: Vec<u8> },
    Hid12In { timestamp_us: u64, data: Vec<u8> },
}

impl UsbPacket {
    #[must_use]
    pub fn timestamp_us(&self) -> u64 {
        match self {
            Self::XInputIn { timestamp_us, .. } | Self::Hid12In { timestamp_us, .. } => {
                *timestamp_us
            }
        }
    }

    #[must_use]
    pub fn to_capture_record(&self) -> CaptureRecord {
        match self {
            Self::XInputIn { timestamp_us, data } => CaptureRecord {
                timestamp_us: *timestamp_us,
                channel: CaptureChannel::Xinput,
                hex: encode_hex(data),
            },
            Self::Hid12In { timestamp_us, data } => CaptureRecord {
                timestamp_us: *timestamp_us,
                channel: CaptureChannel::Hid12,
                hex: encode_hex(data),
            },
        }
    }
}

/// One line in a `.jsonl` Day-1 diagnostic capture file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub timestamp_us: u64,
    pub channel: CaptureChannel,
    pub hex: String,
}

impl CaptureRecord {
    pub fn to_usb_packet(&self) -> Result<UsbPacket, UsbTransportError> {
        let data = decode_hex(&self.hex).map_err(|msg| {
            UsbTransportError::InvalidCapture(format!("invalid hex payload '{}': {msg}", self.hex))
        })?;
        Ok(match self.channel {
            CaptureChannel::Xinput => UsbPacket::XInputIn {
                timestamp_us: self.timestamp_us,
                data,
            },
            CaptureChannel::Hid12 => UsbPacket::Hid12In {
                timestamp_us: self.timestamp_us,
                data,
            },
        })
    }
}

#[derive(Debug, Error)]
pub enum UsbTransportError {
    #[error("no allowlisted GameSir G7 Pro 8K USB device found")]
    NotFound,
    #[error("{}", .0.message)]
    FallbackQuirk(FallbackQuirkInfo),
    #[error("USB device disconnected: {0}")]
    Disconnected(String),
    #[error("USB transport error: {0}")]
    Usb(String),
    #[error("capture parse error: {0}")]
    InvalidCapture(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Abstract USB transport supporting real `nusb` hardware, in-memory mocks, and `.jsonl` replay.
pub trait UsbTransport: Send {
    fn device(&self) -> &MatchedDevice;
    fn has_hid12(&self) -> bool;
    fn recv_packet(&mut self, timeout: Duration) -> Result<Option<UsbPacket>, UsbTransportError>;
    fn send_rumble(&mut self, rumble: RumbleState) -> Result<(), UsbTransportError>;
}

/// Deterministic in-memory mock USB transport for unit & integration tests.
#[derive(Debug)]
pub struct MockUsbTransport {
    device: MatchedDevice,
    has_hid12: bool,
    packets: VecDeque<UsbPacket>,
    sent_rumbles: Vec<RumbleState>,
    disconnect_when_empty: bool,
}

impl MockUsbTransport {
    #[must_use]
    pub fn new(packets: Vec<UsbPacket>) -> Self {
        Self {
            device: MatchedDevice {
                vid: GAMESIR_VENDOR_ID,
                pid: 0x1035,
                edition: DeviceEdition::PlainBlack,
                connection: ConnectionKind::Dongle,
                name: "Mock GameSir G7 Pro 8K PC (2.4GHz Dongle)".to_string(),
                from_extra_pids: false,
            },
            has_hid12: true,
            packets: packets.into(),
            sent_rumbles: Vec::new(),
            disconnect_when_empty: true,
        }
    }

    #[must_use]
    pub fn sent_rumbles(&self) -> &[RumbleState] {
        &self.sent_rumbles
    }
}

impl UsbTransport for MockUsbTransport {
    fn device(&self) -> &MatchedDevice {
        &self.device
    }

    fn has_hid12(&self) -> bool {
        self.has_hid12
    }

    fn recv_packet(&mut self, _timeout: Duration) -> Result<Option<UsbPacket>, UsbTransportError> {
        if let Some(pkt) = self.packets.pop_front() {
            Ok(Some(pkt))
        } else if self.disconnect_when_empty {
            Err(UsbTransportError::Disconnected(
                "mock transport drained".to_string(),
            ))
        } else {
            Ok(None)
        }
    }

    fn send_rumble(&mut self, rumble: RumbleState) -> Result<(), UsbTransportError> {
        self.sent_rumbles.push(rumble);
        Ok(())
    }
}

/// Offline replay transport reading `.jsonl` files recorded by `capture`.
#[derive(Debug)]
pub struct ReplayTransport {
    device: MatchedDevice,
    packets: VecDeque<UsbPacket>,
    realtime: bool,
    last_ts_us: Option<u64>,
    sent_rumbles: Vec<RumbleState>,
}

impl ReplayTransport {
    pub fn from_jsonl_file(path: &Path, realtime: bool) -> Result<Self, UsbTransportError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut packets = VecDeque::new();

        for (idx, line) in reader.lines().enumerate() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let record: CaptureRecord = serde_json::from_str(trimmed).map_err(|err| {
                UsbTransportError::InvalidCapture(format!("line {}: {err}", idx + 1))
            })?;
            packets.push_back(record.to_usb_packet()?);
        }

        Ok(Self {
            device: MatchedDevice {
                vid: GAMESIR_VENDOR_ID,
                pid: 0x1035,
                edition: DeviceEdition::PlainBlack,
                connection: ConnectionKind::Dongle,
                name: format!("Replay Capture ({})", path.display()),
                from_extra_pids: false,
            },
            packets,
            realtime,
            last_ts_us: None,
            sent_rumbles: Vec::new(),
        })
    }

    #[must_use]
    pub fn remaining(&self) -> usize {
        self.packets.len()
    }
}

impl UsbTransport for ReplayTransport {
    fn device(&self) -> &MatchedDevice {
        &self.device
    }

    fn has_hid12(&self) -> bool {
        true
    }

    fn recv_packet(&mut self, _timeout: Duration) -> Result<Option<UsbPacket>, UsbTransportError> {
        let Some(pkt) = self.packets.pop_front() else {
            return Err(UsbTransportError::Disconnected(
                "replay capture finished".to_string(),
            ));
        };

        if self.realtime {
            let ts = pkt.timestamp_us();
            if let Some(prev) = self.last_ts_us {
                let delta_us = ts.saturating_sub(prev).min(250_000);
                if delta_us > 0 {
                    std::thread::sleep(Duration::from_micros(delta_us));
                }
            }
            self.last_ts_us = Some(ts);
        }

        Ok(Some(pkt))
    }

    fn send_rumble(&mut self, rumble: RumbleState) -> Result<(), UsbTransportError> {
        self.sent_rumbles.push(rumble);
        Ok(())
    }
}

#[must_use]
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

pub fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !clean.len().is_multiple_of(2) {
        return Err("odd number of hex characters".to_string());
    }
    let mut out = Vec::with_capacity(clean.len() / 2);
    let bytes = clean.as_bytes();
    for &[hi_ch, lo_ch] in bytes.as_chunks::<2>().0 {
        let hi = hex_nibble(hi_ch)?;
        let lo = hex_nibble(lo_ch)?;
        out.push((hi << 4) | lo);
    }
    Ok(out)
}

fn hex_nibble(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(format!("invalid hex character '{}'", b as char)),
    }
}
