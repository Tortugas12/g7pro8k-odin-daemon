use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use evdev::{AbsInfo, AbsoluteAxisCode, Device, EventSummary, KeyCode};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::device_registry::GAMESIR_VENDOR_ID;
use crate::output::profiles::{VIRTUAL_DS4_NAME, VIRTUAL_PHYS_PREFIX};
use crate::protocol::state::GamepadState;

/// Metadata for a discovered `/dev/input/event*` candidate inspected passively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OdinDeviceCandidate {
    pub event_node: String,
    pub name: String,
    pub phys: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub has_gamepad_buttons: bool,
    pub has_abs_axes: bool,
}

/// Read-only status of the built-in AYN Odin 3 Pro controller.
///
/// `isolated` is ALWAYS `true` because this module NEVER grabs or writes to the device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OdinControllerStatus {
    Active {
        name: String,
        event_node: String,
        isolated: bool,
    },
    NotDetected {
        isolated: bool,
    },
}

impl Default for OdinControllerStatus {
    fn default() -> Self {
        Self::NotDetected { isolated: true }
    }
}

/// Returns `true` if `candidate` belongs to this daemon's virtual `uinput` output or any
/// GameSir `0x3537` device and MUST therefore be excluded from passive Odin 3 monitoring.
#[must_use]
pub fn is_excluded_from_odin_monitor(candidate: &OdinDeviceCandidate) -> bool {
    if candidate.phys.starts_with(VIRTUAL_PHYS_PREFIX) {
        return true;
    }
    if candidate.vendor_id == GAMESIR_VENDOR_ID {
        return true;
    }
    if candidate.name.starts_with("GameSir G7 Pro 8K") {
        return true;
    }
    if candidate.name == VIRTUAL_DS4_NAME
        && candidate.vendor_id == 0x054C
        && candidate.product_id == 0x09CC
        && (candidate.phys.is_empty() || candidate.phys.contains("g7pro8k"))
    {
        return true;
    }
    false
}

/// Selects the best built-in Odin 3 controller candidate while strictly excluding our own
/// virtual devices and GameSir USB controllers.
#[must_use]
pub fn select_odin_builtin(candidates: &[OdinDeviceCandidate]) -> Option<&OdinDeviceCandidate> {
    let eligible: Vec<&OdinDeviceCandidate> = candidates
        .iter()
        .filter(|c| c.has_gamepad_buttons && c.has_abs_axes && !is_excluded_from_odin_monitor(c))
        .collect();

    if eligible.is_empty() {
        return None;
    }

    eligible
        .into_iter()
        .max_by_key(|c| score_odin_candidate(&c.name))
}

fn score_odin_candidate(name: &str) -> u32 {
    let lower = name.to_ascii_lowercase();
    if lower.contains("odin") || lower.contains("ayn") {
        100
    } else if lower.contains("xbox") || lower.contains("gamepad") {
        60
    } else if lower.contains("controller") || lower.contains("joystick") {
        40
    } else {
        10
    }
}

/// Strictly passive, read-only (`O_RDONLY`, ZERO `EVIOCGRAB`) observer for the AYN Odin 3 Pro's
/// built-in controller so the companion Web UI can show a live read-only Odin 3 handheld SVG.
pub struct OdinMonitor {
    device: Option<Device>,
    status: OdinControllerStatus,
    state: GamepadState,
    abs_ranges: HashMap<AbsoluteAxisCode, AbsInfo>,
}

impl Default for OdinMonitor {
    fn default() -> Self {
        Self::new()
    }
}

impl OdinMonitor {
    /// Creates an unattached passive monitor and performs an initial scan.
    #[must_use]
    pub fn new() -> Self {
        let mut monitor = Self {
            device: None,
            status: OdinControllerStatus::NotDetected { isolated: true },
            state: GamepadState::default(),
            abs_ranges: HashMap::new(),
        };
        monitor.rescan();
        monitor
    }

    #[must_use]
    pub fn status(&self) -> OdinControllerStatus {
        self.status.clone()
    }

    #[must_use]
    pub fn state(&self) -> GamepadState {
        self.state
    }

    /// Scans `/sys/class/input/event*` and opens the built-in Odin 3 controller strictly read-only.
    ///
    /// Safety invariant: NEVER calls `EVIOCGRAB` or writes to any `/dev/input/event*` node.
    pub fn rescan(&mut self) {
        if self.device.is_some() {
            return;
        }

        let candidates = scan_input_candidates(Path::new("/sys/class/input"));
        let Some(chosen) = select_odin_builtin(&candidates) else {
            self.status = OdinControllerStatus::NotDetected { isolated: true };
            return;
        };

        // Strictly passive read-only open — NEVER call `.grab()`!
        match Device::open(&chosen.event_node) {
            Ok(dev) => {
                if let Err(err) = set_nonblocking(dev.as_raw_fd()) {
                    warn!(node = %chosen.event_node, error = %err, "failed to set O_NONBLOCK on passive Odin monitor");
                    return;
                }

                let mut abs_ranges = HashMap::new();
                if let Ok(abs_state) = dev.get_abs_state() {
                    for (idx, info) in abs_state.iter().enumerate() {
                        if info.maximum > info.minimum {
                            abs_ranges.insert(
                                AbsoluteAxisCode(idx as u16),
                                AbsInfo::new(
                                    info.value,
                                    info.minimum,
                                    info.maximum,
                                    info.fuzz,
                                    info.flat,
                                    info.resolution,
                                ),
                            );
                        }
                    }
                }

                debug!(
                    name = %chosen.name,
                    event_node = %chosen.event_node,
                    "attached passive read-only Odin 3 monitor (non-exclusive)"
                );
                self.status = OdinControllerStatus::Active {
                    name: chosen.name.clone(),
                    event_node: chosen.event_node.clone(),
                    isolated: true,
                };
                self.abs_ranges = abs_ranges;
                self.device = Some(dev);
            }
            Err(err) => {
                debug!(
                    node = %chosen.event_node,
                    error = %err,
                    "unable to open built-in controller read-only; reporting NotDetected"
                );
                self.status = OdinControllerStatus::NotDetected { isolated: true };
            }
        }
    }

    /// Drains any pending non-blocking read-only events from the built-in controller.
    pub fn poll(&mut self) -> GamepadState {
        let mut detached = false;
        if let Some(dev) = self.device.as_mut() {
            match dev.fetch_events() {
                Ok(events) => {
                    for ev in events {
                        match ev.destructure() {
                            EventSummary::Key(_, code, val) => {
                                apply_key_event(&mut self.state, code, val != 0);
                            }
                            EventSummary::AbsoluteAxis(_, code, val) => {
                                let info = self.abs_ranges.get(&code).copied();
                                apply_abs_event(&mut self.state, code, val, info);
                            }
                            _ => {}
                        }
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
                Err(err) => {
                    debug!(error = %err, "passive Odin 3 monitor detached");
                    detached = true;
                }
            }
        }

        if detached {
            self.device = None;
            self.state = GamepadState::default();
            self.status = OdinControllerStatus::NotDetected { isolated: true };
        }

        self.state
    }
}

fn scan_input_candidates(sys_class_input: &Path) -> Vec<OdinDeviceCandidate> {
    let Ok(entries) = fs::read_dir(sys_class_input) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(name_str) = file_name.to_str() else {
            continue;
        };
        if !name_str.starts_with("event") {
            continue;
        }

        let dev_dir = entry.path().join("device");
        let name = read_trimmed(&dev_dir.join("name")).unwrap_or_default();
        let phys = read_trimmed(&dev_dir.join("phys")).unwrap_or_default();
        let vendor_id = read_hex_u16(&dev_dir.join("id/vendor")).unwrap_or(0);
        let product_id = read_hex_u16(&dev_dir.join("id/product")).unwrap_or(0);
        let event_node = PathBuf::from("/dev/input").join(name_str);

        let prelim = OdinDeviceCandidate {
            event_node: event_node.to_string_lossy().into_owned(),
            name,
            phys,
            vendor_id,
            product_id,
            has_gamepad_buttons: false,
            has_abs_axes: false,
        };

        // Fast-path exclusion before touching `/dev/input/event*`.
        if is_excluded_from_odin_monitor(&prelim) {
            continue;
        }

        // Open read-only solely to inspect evdev capability bits (NEVER grab!).
        let Ok(dev) = Device::open(&event_node) else {
            continue;
        };
        let has_gamepad_buttons = dev.supported_keys().is_some_and(|keys| {
            keys.contains(KeyCode::BTN_SOUTH)
                && keys.contains(KeyCode::BTN_EAST)
                && keys.contains(KeyCode::BTN_WEST)
        });
        let has_abs_axes = dev.supported_absolute_axes().is_some_and(|axes| {
            axes.contains(AbsoluteAxisCode::ABS_X) && axes.contains(AbsoluteAxisCode::ABS_Y)
        });

        out.push(OdinDeviceCandidate {
            has_gamepad_buttons,
            has_abs_axes,
            ..prelim
        });
    }

    out
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_hex_u16(path: &Path) -> Option<u16> {
    let raw = read_trimmed(path)?;
    u16::from_str_radix(raw.trim_start_matches("0x"), 16).ok()
}

fn apply_key_event(state: &mut GamepadState, code: KeyCode, pressed: bool) {
    match code {
        KeyCode::BTN_SOUTH => state.buttons.a = pressed,
        KeyCode::BTN_EAST => state.buttons.b = pressed,
        KeyCode::BTN_WEST => state.buttons.x = pressed,
        KeyCode::BTN_NORTH => state.buttons.y = pressed,
        KeyCode::BTN_TL => state.buttons.lb = pressed,
        KeyCode::BTN_TR => state.buttons.rb = pressed,
        KeyCode::BTN_SELECT => state.buttons.view = pressed,
        KeyCode::BTN_START => state.buttons.menu = pressed,
        KeyCode::BTN_MODE => state.buttons.guide = pressed,
        KeyCode::BTN_THUMBL => state.buttons.ls_click = pressed,
        KeyCode::BTN_THUMBR => state.buttons.rs_click = pressed,
        KeyCode::BTN_DPAD_UP => state.buttons.dpad_up = pressed,
        KeyCode::BTN_DPAD_DOWN => state.buttons.dpad_down = pressed,
        KeyCode::BTN_DPAD_LEFT => state.buttons.dpad_left = pressed,
        KeyCode::BTN_DPAD_RIGHT => state.buttons.dpad_right = pressed,
        _ => {}
    }
}

fn apply_abs_event(
    state: &mut GamepadState,
    code: AbsoluteAxisCode,
    value: i32,
    info: Option<AbsInfo>,
) {
    match code {
        AbsoluteAxisCode::ABS_X => state.lx = normalize_stick(value, info, false),
        AbsoluteAxisCode::ABS_Y => state.ly = normalize_stick(value, info, true),
        AbsoluteAxisCode::ABS_RX => state.rx = normalize_stick(value, info, false),
        AbsoluteAxisCode::ABS_RY => state.ry = normalize_stick(value, info, true),
        AbsoluteAxisCode::ABS_Z | AbsoluteAxisCode::ABS_BRAKE => {
            state.lt = normalize_trigger(value, info);
        }
        AbsoluteAxisCode::ABS_RZ | AbsoluteAxisCode::ABS_GAS => {
            state.rt = normalize_trigger(value, info);
        }
        AbsoluteAxisCode::ABS_HAT0X => {
            state.buttons.dpad_left = value < 0;
            state.buttons.dpad_right = value > 0;
        }
        AbsoluteAxisCode::ABS_HAT0Y => {
            state.buttons.dpad_up = value < 0;
            state.buttons.dpad_down = value > 0;
        }
        _ => {}
    }
}

fn normalize_stick(value: i32, info: Option<AbsInfo>, invert_y: bool) -> i16 {
    let norm = if let Some(abs) = info {
        let span = f64::from(abs.maximum() - abs.minimum());
        if span > 0.0 {
            let centered = ((f64::from(value - abs.minimum()) / span) * 2.0) - 1.0;
            centered.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    } else {
        (f64::from(value) / 32768.0).clamp(-1.0, 1.0)
    };

    let signed = if invert_y { -norm } else { norm };
    (signed * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}

fn normalize_trigger(value: i32, info: Option<AbsInfo>) -> u8 {
    if let Some(abs) = info {
        let span = f64::from(abs.maximum() - abs.minimum());
        if span > 0.0 {
            let pct = (f64::from(value - abs.minimum()) / span).clamp(0.0, 1.0);
            return (pct * 255.0).round() as u8;
        }
    }
    value.clamp(0, 255) as u8
}

fn set_nonblocking(fd: i32) -> io::Result<()> {
    // SAFETY: Toggles O_NONBLOCK on an open read-only evdev file descriptor.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
