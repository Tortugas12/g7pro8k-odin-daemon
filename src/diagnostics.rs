use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::coalescer::Coalescer;
use crate::config::Config;
use crate::device_registry::{DeviceMatch, FallbackQuirkInfo};
use crate::gyro_dsu::{GyroDsuStub, GyroSample};
use crate::odin_monitor::{OdinControllerStatus, OdinMonitor};
use crate::output::uinput_backend::UinputVirtualOutput;
use crate::output::{MockVirtualOutput, OutputDispatcher, OutputError, VirtualOutput};
use crate::protocol::hid12::{parse_hid12_report, Hid12ParseOutcome};
use crate::protocol::state::GamepadState;
use crate::protocol::xinput::parse_xinput_report;
use crate::transport::nusb_backend::{probe_usb_devices, NusbTransport};
use crate::transport::{
    CaptureRecord, ReplayTransport, UsbPacket, UsbTransport, UsbTransportError,
};

pub const MAX_CAPTURE_SECONDS: u64 = 30;
pub const DEFAULT_CAPTURE_SECONDS: u64 = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeEndpointEntry {
    pub address: String,
    pub direction: String,
    pub transfer_type: String,
    pub max_packet_size: usize,
    pub interval: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeInterfaceEntry {
    pub interface_number: u8,
    pub class: String,
    pub subclass: String,
    pub protocol: String,
    pub endpoints: Vec<ProbeEndpointEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeDeviceEntry {
    pub bus_id: String,
    pub device_address: u8,
    pub vid: String,
    pub pid: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub classification: DeviceMatch,
    pub interfaces: Vec<ProbeInterfaceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub gamesir_count: usize,
    pub quirk_warning: Option<FallbackQuirkInfo>,
    pub devices: Vec<ProbeDeviceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub healthy: bool,
    pub checks: Vec<DoctorCheck>,
    pub odin3_status: OdinControllerStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaySummary {
    pub packets_processed: u64,
    pub xinput_packets: u64,
    pub hid12_packets: u64,
    pub emitted_frames: u64,
    pub immediate_button_flushes: u64,
    pub final_state: GamepadState,
}

#[derive(Debug, Error)]
pub enum DiagnosticError {
    #[error(transparent)]
    Usb(#[from] UsbTransportError),
    #[error(transparent)]
    Output(#[from] OutputError),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("another daemon instance is already running (pid {0})")]
    AlreadyRunning(u32),
}

/// Runs a non-invasive system health check (`doctor`).
#[must_use]
pub fn run_doctor(config: &Config) -> DoctorReport {
    let mut checks = Vec::new();

    // 1. Config validity.
    match config.validate() {
        Ok(()) => checks.push(DoctorCheck {
            name: "config".to_string(),
            ok: true,
            detail: format!(
                "valid (profile={:?}, emit_rate_hz={}, web={})",
                config.profile, config.emit_rate_hz, config.web_ui.bind
            ),
        }),
        Err(err) => checks.push(DoctorCheck {
            name: "config".to_string(),
            ok: false,
            detail: err.to_string(),
        }),
    }

    // 2. Effective UID (root required on Android for /dev/uinput and usbdevfs).
    // SAFETY: `geteuid` is always safe to call on Linux.
    let euid = unsafe { libc::geteuid() };
    checks.push(DoctorCheck {
        name: "privileges".to_string(),
        ok: euid == 0,
        detail: if euid == 0 {
            "running as root (euid=0)".to_string()
        } else {
            format!("running non-root (euid={euid}); root is required on Odin 3 for /dev/uinput and USB")
        },
    });

    // 3. /dev/uinput presence & access.
    let uinput_path = Path::new("/dev/uinput");
    let uinput_ok = fs::OpenOptions::new().write(true).open(uinput_path).is_ok();
    checks.push(DoctorCheck {
        name: "uinput".to_string(),
        ok: uinput_ok,
        detail: if uinput_ok {
            "/dev/uinput is writable".to_string()
        } else if uinput_path.exists() {
            "/dev/uinput exists but is not writable by current user".to_string()
        } else {
            "/dev/uinput not found (kernel CONFIG_INPUT_UINPUT required)".to_string()
        },
    });

    // 4. USB enumeration & GameSir detection.
    match probe_usb_devices(&config.extra_pids) {
        Ok(report) => {
            if let Some(quirk) = report.quirk_warning {
                checks.push(DoctorCheck {
                    name: "usb_gamesir".to_string(),
                    ok: false,
                    detail: quirk.message,
                });
            } else if report.gamesir_count > 0 {
                checks.push(DoctorCheck {
                    name: "usb_gamesir".to_string(),
                    ok: true,
                    detail: format!(
                        "detected {} allowlisted GameSir G7 Pro 8K device(s)",
                        report.gamesir_count
                    ),
                });
            } else {
                checks.push(DoctorCheck {
                    name: "usb_gamesir".to_string(),
                    ok: true,
                    detail: "no GameSir G7 Pro 8K currently plugged in (USB bus accessible)"
                        .to_string(),
                });
            }
        }
        Err(err) => checks.push(DoctorCheck {
            name: "usb_gamesir".to_string(),
            ok: false,
            detail: err.to_string(),
        }),
    }

    // 5. Passive read-only Odin 3 built-in controller check (NEVER grabs).
    let monitor = OdinMonitor::new();
    let odin3_status = monitor.status();
    checks.push(DoctorCheck {
        name: "odin3_isolation".to_string(),
        ok: true,
        detail: match &odin3_status {
            OdinControllerStatus::Active {
                name, event_node, ..
            } => format!(
                "built-in controller '{name}' ({event_node}) observed read-only (untouched)"
            ),
            OdinControllerStatus::NotDetected { .. } => {
                "no built-in Odin 3 gamepad detected on this host (isolated=true)".to_string()
            }
        },
    });

    let healthy = checks.iter().all(|c| c.ok);
    DoctorReport {
        healthy,
        checks,
        odin3_status,
    }
}

/// Captures raw USB packets from a connected GameSir G7 Pro 8K into `.jsonl` format.
pub fn capture_from_usb(
    config: &Config,
    duration_secs: u64,
) -> Result<Vec<CaptureRecord>, DiagnosticError> {
    let bounded_secs = duration_secs.clamp(1, MAX_CAPTURE_SECONDS);
    let mut transport = NusbTransport::open_first(&config.extra_pids)?;
    let deadline = Instant::now() + Duration::from_secs(bounded_secs);
    let mut records = Vec::new();

    while Instant::now() < deadline {
        match transport.recv_packet(Duration::from_millis(50)) {
            Ok(Some(pkt)) => records.push(pkt.to_capture_record()),
            Ok(None) => {}
            Err(UsbTransportError::Disconnected(_)) => break,
            Err(err) => return Err(DiagnosticError::Usb(err)),
        }
    }

    Ok(records)
}

/// Serializes capture records to newline-delimited JSON (`.jsonl`).
pub fn format_capture_jsonl(records: &[CaptureRecord]) -> Result<String, DiagnosticError> {
    let mut out = String::new();
    for rec in records {
        out.push_str(&serde_json::to_string(rec)?);
        out.push('\n');
    }
    Ok(out)
}

/// Replays a `.jsonl` capture file through the full protocol + coalescer + output pipeline.
pub fn replay_capture_file(
    input_path: &Path,
    config: &Config,
    dry_run: bool,
    realtime: bool,
    enable_gyro_dsu: bool,
) -> Result<ReplaySummary, DiagnosticError> {
    let mut transport = ReplayTransport::from_jsonl_file(input_path, realtime)?;
    if dry_run {
        let output = MockVirtualOutput::new(config.profile);
        replay_with_output(&mut transport, output, config, enable_gyro_dsu)
    } else {
        let output = UinputVirtualOutput::new(config.profile)?;
        replay_with_output(&mut transport, output, config, enable_gyro_dsu)
    }
}

/// Generic headless replay runner over any [`UsbTransport`] and [`VirtualOutput`].
pub fn replay_with_output<T: UsbTransport, O: VirtualOutput>(
    transport: &mut T,
    output: O,
    config: &Config,
    enable_gyro_dsu: bool,
) -> Result<ReplaySummary, DiagnosticError> {
    let mut coalescer = Coalescer::new(config.emit_rate_hz);
    let mut dispatcher = OutputDispatcher::new(output, config.extra_buttons.clone());
    let mut gyro = GyroDsuStub::new(enable_gyro_dsu);
    let mut state = GamepadState::default();

    let mut packets_processed = 0u64;
    let mut xinput_packets = 0u64;
    let mut hid12_packets = 0u64;
    let mut last_ts_us = 0u64;

    loop {
        match transport.recv_packet(Duration::from_millis(10)) {
            Ok(Some(pkt)) => {
                packets_processed += 1;
                last_ts_us = pkt.timestamp_us();
                match pkt {
                    UsbPacket::XInputIn { data, .. } => {
                        xinput_packets += 1;
                        if let Ok(report) = parse_xinput_report(&data) {
                            report.apply_to(&mut state);
                        }
                    }
                    UsbPacket::Hid12In { timestamp_us, data } => {
                        hid12_packets += 1;
                        if let Hid12ParseOutcome::Parsed(frame) =
                            parse_hid12_report(&data, &config.hid12_layout)
                        {
                            state.extra = frame.extra;
                            if let Some(raw_imu) = frame.gyro_raw {
                                gyro.submit(GyroSample::from_raw_imu(timestamp_us, raw_imu));
                            }
                        }
                    }
                }

                if let Some(emitted) = coalescer.push(state, last_ts_us) {
                    dispatcher.emit_state(&emitted)?;
                }
            }
            Ok(None) => {}
            Err(UsbTransportError::Disconnected(_)) => break,
            Err(err) => return Err(DiagnosticError::Usb(err)),
        }
    }

    if let Some(flushed) = coalescer.flush_due(last_ts_us.saturating_add(10_000)) {
        dispatcher.emit_state(&flushed)?;
    }
    dispatcher.release_all()?;

    let stats = coalescer.stats();
    Ok(ReplaySummary {
        packets_processed,
        xinput_packets,
        hid12_packets,
        emitted_frames: stats.emitted,
        immediate_button_flushes: stats.immediate_button_flushes,
        final_state: state,
    })
}

/// RAII single-instance PID file guard.
pub struct PidFileGuard {
    path: PathBuf,
}

impl PidFileGuard {
    pub fn acquire(path: &Path) -> Result<Self, DiagnosticError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        if let Ok(existing) = fs::read_to_string(path) {
            if let Ok(pid) = existing.trim().parse::<u32>() {
                if pid != std::process::id() && process_is_alive(pid) {
                    return Err(DiagnosticError::AlreadyRunning(pid));
                }
            }
        }

        let mut file = File::create(path)?;
        writeln!(file, "{}", std::process::id())?;
        file.sync_all()?;

        Ok(Self {
            path: path.to_path_buf(),
        })
    }
}

impl Drop for PidFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn process_is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}
