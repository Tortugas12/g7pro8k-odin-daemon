use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::coalescer::Coalescer;
use crate::config::{Config, Profile};
use crate::device_registry::{ConnectionKind, FallbackQuirkInfo, MatchedDevice};
use crate::gyro_dsu::{GyroDsuStub, GyroSample};
use crate::odin_monitor::{OdinControllerStatus, OdinMonitor};
use crate::output::uinput_backend::UinputVirtualOutput;
use crate::output::{MockVirtualOutput, OutputDispatcher};
use crate::protocol::hid12::{parse_hid12_report, Hid12ParseOutcome};
use crate::protocol::state::{
    ActiveSource, ConnectionState, ControllerTelemetry, GamepadState, RumbleState,
    TelemetrySnapshot,
};
use crate::protocol::xinput::parse_xinput_report;
use crate::transport::nusb_backend::NusbTransport;
use crate::transport::{CaptureRecord, UsbPacket, UsbTransport, UsbTransportError};

/// Control commands sent from CLI signals or the Phase 4 Web UI to the daemon loop.
#[derive(Debug, Clone)]
pub enum DaemonCommand {
    ReloadConfig(Box<Config>),
    Pause,
    Resume,
    RumbleTest {
        large_motor: u8,
        small_motor: u8,
        duration_ms: u64,
    },
    Shutdown,
}

/// Snapshot returned by `GET /api/status` and CLI status queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatusSnapshot {
    pub connection_state: ConnectionState,
    pub detected_device: Option<MatchedDevice>,
    pub quirk_warning: Option<FallbackQuirkInfo>,
    pub profile: Profile,
    pub usb_hz: u32,
    pub emit_hz: u32,
    pub paused: bool,
    pub dry_run: bool,
    pub odin3: OdinControllerStatus,
}

/// Shared state safely accessed by both the daemon loop and the local Web UI.
pub struct SharedDaemonState {
    pub config_path: PathBuf,
    pub config: RwLock<Config>,
    pub status: RwLock<DaemonStatusSnapshot>,
    pub telemetry: RwLock<TelemetrySnapshot>,
    pub capture_tap: Mutex<Option<mpsc::Sender<CaptureRecord>>>,
    pub cmd_tx: mpsc::UnboundedSender<DaemonCommand>,
}

impl SharedDaemonState {
    #[must_use]
    pub fn new(
        config_path: PathBuf,
        config: Config,
        dry_run: bool,
    ) -> (Arc<Self>, mpsc::UnboundedReceiver<DaemonCommand>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let initial_profile = config.profile;

        let shared = Arc::new(Self {
            config_path,
            config: RwLock::new(config),
            status: RwLock::new(DaemonStatusSnapshot {
                connection_state: ConnectionState::Disconnected,
                detected_device: None,
                quirk_warning: None,
                profile: initial_profile,
                usb_hz: 0,
                emit_hz: 0,
                paused: false,
                dry_run,
                odin3: OdinControllerStatus::NotDetected { isolated: true },
            }),
            telemetry: RwLock::new(TelemetrySnapshot {
                profile: initial_profile,
                ..Default::default()
            }),
            capture_tap: Mutex::new(None),
            cmd_tx,
        });

        (shared, cmd_rx)
    }

    pub fn current_config(&self) -> Config {
        self.config.read().expect("config lock poisoned").clone()
    }

    pub fn current_status(&self) -> DaemonStatusSnapshot {
        self.status.read().expect("status lock poisoned").clone()
    }

    pub fn current_telemetry(&self) -> TelemetrySnapshot {
        self.telemetry
            .read()
            .expect("telemetry lock poisoned")
            .clone()
    }

    pub fn send_command(&self, cmd: DaemonCommand) {
        let _ = self.cmd_tx.send(cmd);
    }

    /// Registers a temporary capture tap to record live packets without detaching USB.
    pub fn set_capture_tap(&self, tap: Option<mpsc::Sender<CaptureRecord>>) {
        let mut guard = self.capture_tap.lock().expect("capture tap lock poisoned");
        *guard = tap;
    }

    fn tap_packet(&self, pkt: &UsbPacket) {
        let mut guard = self.capture_tap.lock().expect("capture tap lock poisoned");
        if let Some(tx) = guard.as_ref() {
            if tx.try_send(pkt.to_capture_record()).is_err() {
                // Drop tap automatically once receiver closes.
                if tx.is_closed() {
                    *guard = None;
                }
            }
        }
    }
}

enum ActiveOutput {
    Real(OutputDispatcher<UinputVirtualOutput>),
    DryRun(OutputDispatcher<MockVirtualOutput>),
}

impl ActiveOutput {
    fn create(config: &Config, dry_run: bool) -> Result<Self, crate::output::OutputError> {
        if dry_run {
            Ok(Self::DryRun(OutputDispatcher::new(
                MockVirtualOutput::new(config.profile),
                config.extra_buttons.clone(),
            )))
        } else {
            let uinput = UinputVirtualOutput::new(config.profile)?;
            Ok(Self::Real(OutputDispatcher::new(
                uinput,
                config.extra_buttons.clone(),
            )))
        }
    }

    fn emit_state(&mut self, state: &GamepadState) -> Result<(), crate::output::OutputError> {
        match self {
            Self::Real(d) => d.emit_state(state),
            Self::DryRun(d) => d.emit_state(state),
        }
    }

    fn poll_rumble(&mut self) -> Result<Option<RumbleState>, crate::output::OutputError> {
        match self {
            Self::Real(d) => d.poll_rumble(),
            Self::DryRun(d) => d.poll_rumble(),
        }
    }

    fn update_extra_buttons(
        &mut self,
        extra: crate::config::ExtraButtonsConfig,
    ) -> Result<(), crate::output::OutputError> {
        match self {
            Self::Real(d) => d.update_extra_buttons(extra),
            Self::DryRun(d) => d.update_extra_buttons(extra),
        }
    }

    fn release_all(&mut self) {
        match self {
            Self::Real(d) => {
                let _ = d.release_all();
            }
            Self::DryRun(d) => {
                let _ = d.release_all();
            }
        }
    }
}

/// Runs the blocking hardware + Odin monitor daemon loop until `DaemonCommand::Shutdown`.
pub fn run_daemon_loop(
    shared: Arc<SharedDaemonState>,
    mut cmd_rx: mpsc::UnboundedReceiver<DaemonCommand>,
    dry_run: bool,
    enable_gyro_dsu: bool,
) {
    let mut config = shared.current_config();
    let mut coalescer = Coalescer::new(config.emit_rate_hz);
    let mut gyro = GyroDsuStub::new(enable_gyro_dsu);
    let mut odin_monitor = OdinMonitor::new();

    let mut transport: Option<NusbTransport> = None;
    let mut output: Option<ActiveOutput> = None;
    let mut gamesir_state = GamepadState::default();
    let mut paused = false;
    let mut quirk_warning: Option<FallbackQuirkInfo> = None;

    let mut last_connect_attempt = Instant::now() - Duration::from_secs(5);
    let mut last_odin_scan = Instant::now();
    let mut rate_window_start = Instant::now();
    let mut usb_pkts_in_window = 0u32;
    let mut emitted_in_window = 0u32;
    let mut measured_usb_hz = 0u32;
    let mut measured_emit_hz = 0u32;
    let mut rumble_test_stop_at: Option<Instant> = None;

    let epoch = Instant::now();

    info!(
        profile = ?config.profile,
        emit_rate_hz = config.emit_rate_hz,
        dry_run,
        "g7pro8k-odin-daemon core loop started"
    );

    loop {
        // 1. Process control commands (hot-reload, pause/resume, rumble test, shutdown).
        while let Ok(cmd) = cmd_rx.try_recv() {
            match cmd {
                DaemonCommand::Shutdown => {
                    info!("shutdown requested; releasing virtual devices and USB transport");
                    if let Some(mut out) = output.take() {
                        out.release_all();
                    }
                    drop(transport);
                    return;
                }
                DaemonCommand::Pause => {
                    if !paused {
                        info!("pausing daemon: releasing virtual output and USB device");
                        paused = true;
                        if let Some(mut out) = output.take() {
                            out.release_all();
                        }
                        transport = None;
                        gamesir_state = GamepadState::default();
                        coalescer.reset();
                        measured_usb_hz = 0;
                        measured_emit_hz = 0;
                    }
                }
                DaemonCommand::Resume => {
                    if paused {
                        info!("resuming daemon");
                        paused = false;
                        last_connect_attempt = Instant::now() - Duration::from_secs(5);
                    }
                }
                DaemonCommand::ReloadConfig(boxed_cfg) => {
                    let new_cfg = *boxed_cfg;
                    let profile_changed = new_cfg.profile != config.profile;
                    info!(
                        profile = ?new_cfg.profile,
                        emit_rate_hz = new_cfg.emit_rate_hz,
                        profile_changed,
                        "hot-reloading daemon configuration"
                    );
                    coalescer.set_emit_rate_hz(new_cfg.emit_rate_hz);
                    if profile_changed {
                        // Cleanly recreate uinput devices on profile swap.
                        if let Some(mut old) = output.take() {
                            old.release_all();
                        }
                        if transport.is_some() && !paused {
                            match ActiveOutput::create(&new_cfg, dry_run) {
                                Ok(new_out) => output = Some(new_out),
                                Err(err) => {
                                    warn!(error = %err, "failed to recreate uinput on profile switch")
                                }
                            }
                        }
                    } else if let Some(out) = output.as_mut() {
                        let _ = out.update_extra_buttons(new_cfg.extra_buttons.clone());
                    }

                    if !new_cfg.enable_rumble {
                        if let Some(t) = transport.as_mut() {
                            let _ = t.send_rumble(RumbleState::default());
                        }
                    }

                    config = new_cfg.clone();
                    *shared.config.write().expect("config lock") = new_cfg;
                }
                DaemonCommand::RumbleTest {
                    large_motor,
                    small_motor,
                    duration_ms,
                } => {
                    if let Some(t) = transport.as_mut() {
                        let _ = t.send_rumble(RumbleState {
                            large_motor,
                            small_motor,
                        });
                        rumble_test_stop_at =
                            Some(Instant::now() + Duration::from_millis(duration_ms.min(1000)));
                    }
                }
            }
        }

        // Stop active rumble test once its bounded window expires.
        if let Some(stop_at) = rumble_test_stop_at {
            if Instant::now() >= stop_at {
                rumble_test_stop_at = None;
                if let Some(t) = transport.as_mut() {
                    let _ = t.send_rumble(RumbleState::default());
                }
            }
        }

        // 2. Passive read-only Odin 3 built-in controller poll (NEVER grabs).
        if last_odin_scan.elapsed() >= Duration::from_secs(2) {
            odin_monitor.rescan();
            last_odin_scan = Instant::now();
        }
        let odin3_state = odin_monitor.poll();

        // 3. Hotplug / reconnect GameSir G7 Pro 8K when not paused.
        if !paused
            && transport.is_none()
            && last_connect_attempt.elapsed() >= Duration::from_millis(500)
        {
            last_connect_attempt = Instant::now();
            match NusbTransport::open_first(&config.extra_pids) {
                Ok(usb) => {
                    quirk_warning = None;
                    match ActiveOutput::create(&config, dry_run) {
                        Ok(out) => {
                            info!(device = %usb.device().name, "GameSir G7 Pro 8K connected and virtual output active");
                            transport = Some(usb);
                            output = Some(out);
                            coalescer.reset();
                            gamesir_state = GamepadState::default();
                        }
                        Err(err) => {
                            warn!(error = %err, "failed to create virtual uinput output");
                        }
                    }
                }
                Err(UsbTransportError::FallbackQuirk(quirk)) => {
                    if quirk_warning.is_none() {
                        warn!("{}", quirk.message);
                    }
                    quirk_warning = Some(quirk);
                }
                Err(UsbTransportError::NotFound) => {
                    quirk_warning = None;
                }
                Err(err) => {
                    debug!(error = %err, "USB connect retry pending");
                }
            }
        }

        // 4. Pump incoming USB packets and coalesce to virtual uinput output.
        if let Some(usb) = transport.as_mut() {
            let mut disconnected = false;
            match usb.recv_packet(Duration::from_millis(8)) {
                Ok(Some(pkt)) => {
                    shared.tap_packet(&pkt);
                    usb_pkts_in_window += 1;
                    let ts_us = epoch.elapsed().as_micros() as u64;

                    match pkt {
                        UsbPacket::XInputIn { data, .. } => {
                            if let Ok(report) = parse_xinput_report(&data) {
                                report.apply_to(&mut gamesir_state);
                            }
                        }
                        UsbPacket::Hid12In { timestamp_us, data } => {
                            if let Hid12ParseOutcome::Parsed(frame) =
                                parse_hid12_report(&data, &config.hid12_layout)
                            {
                                gamesir_state.extra = frame.extra;
                                if let Some(raw_imu) = frame.gyro_raw {
                                    gyro.submit(GyroSample::from_raw_imu(timestamp_us, raw_imu));
                                }
                            }
                        }
                    }

                    if let Some(frame) = coalescer.push(gamesir_state, ts_us) {
                        if let Some(out) = output.as_mut() {
                            if out.emit_state(&frame).is_ok() {
                                emitted_in_window += 1;
                            }
                        }
                    }
                }
                Ok(None) => {
                    let ts_us = epoch.elapsed().as_micros() as u64;
                    if let Some(frame) = coalescer.flush_due(ts_us) {
                        if let Some(out) = output.as_mut() {
                            if out.emit_state(&frame).is_ok() {
                                emitted_in_window += 1;
                            }
                        }
                    }
                }
                Err(UsbTransportError::Disconnected(reason)) => {
                    info!(reason = %reason, "GameSir G7 Pro 8K disconnected; cleaning up virtual output");
                    disconnected = true;
                }
                Err(err) => {
                    warn!(error = %err, "USB receive error; reconnecting");
                    disconnected = true;
                }
            }

            // Forward FF rumble events from Android games to the GameSir G7 Pro 8K.
            if !disconnected && rumble_test_stop_at.is_none() {
                if let Some(out) = output.as_mut() {
                    if let Ok(Some(rumble)) = out.poll_rumble() {
                        if config.enable_rumble {
                            let _ = usb.send_rumble(rumble);
                        }
                    }
                }
            }

            if disconnected {
                if let Some(mut out) = output.take() {
                    out.release_all();
                }
                transport = None;
                gamesir_state = GamepadState::default();
                coalescer.reset();
                measured_usb_hz = 0;
                measured_emit_hz = 0;
            }
        } else {
            // No active USB device; sleep gently while keeping Odin 3 live UI responsive (~60Hz).
            std::thread::sleep(Duration::from_millis(16));
        }

        // 5. Update measured Hz every 500ms.
        let elapsed = rate_window_start.elapsed();
        if elapsed >= Duration::from_millis(500) {
            let secs = elapsed.as_secs_f64();
            measured_usb_hz = (f64::from(usb_pkts_in_window) / secs).round() as u32;
            measured_emit_hz = (f64::from(emitted_in_window) / secs).round() as u32;
            usb_pkts_in_window = 0;
            emitted_in_window = 0;
            rate_window_start = Instant::now();
        }

        // 6. Publish status + telemetry snapshots for Web UI / API.
        let connection_state = if paused {
            ConnectionState::Paused
        } else if let Some(usb) = transport.as_ref() {
            match usb.device().connection {
                ConnectionKind::Dongle => ConnectionState::ConnectedDongle,
                ConnectionKind::Wired | ConnectionKind::Custom => ConnectionState::ConnectedWired,
            }
        } else if quirk_warning.is_some() {
            ConnectionState::Quirk10b8
        } else {
            ConnectionState::Disconnected
        };

        let active_source = if matches!(
            connection_state,
            ConnectionState::ConnectedWired | ConnectionState::ConnectedDongle
        ) {
            ActiveSource::Gamesir
        } else {
            ActiveSource::Odin3Builtin
        };

        {
            let mut st = shared.status.write().expect("status lock");
            st.connection_state = connection_state;
            st.detected_device = transport.as_ref().map(|t| t.device().clone());
            st.quirk_warning = quirk_warning.clone();
            st.profile = config.profile;
            st.usb_hz = measured_usb_hz;
            st.emit_hz = measured_emit_hz;
            st.paused = paused;
            st.dry_run = dry_run;
            st.odin3 = odin_monitor.status();
        }

        {
            let mut tel = shared.telemetry.write().expect("telemetry lock");
            tel.active_source = active_source;
            tel.connection_state = connection_state;
            tel.profile = config.profile;
            tel.usb_hz = measured_usb_hz;
            tel.emit_hz = measured_emit_hz;
            tel.gamesir = ControllerTelemetry::from(&gamesir_state);
            tel.odin3 = ControllerTelemetry::from(&odin3_state);
        }
    }
}
