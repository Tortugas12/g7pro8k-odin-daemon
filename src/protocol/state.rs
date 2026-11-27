use serde::{Deserialize, Serialize};

use crate::config::{GamepadButton, Profile};

/// Standard digital gamepad buttons.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GamepadButtons {
    pub a: bool,
    pub b: bool,
    pub x: bool,
    pub y: bool,
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub lb: bool,
    pub rb: bool,
    pub view: bool,
    pub menu: bool,
    pub guide: bool,
    pub ls_click: bool,
    pub rs_click: bool,
}

impl GamepadButtons {
    #[must_use]
    pub fn dpad_hat_x(&self) -> i32 {
        match (self.dpad_left, self.dpad_right) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        }
    }

    #[must_use]
    pub fn dpad_hat_y(&self) -> i32 {
        match (self.dpad_up, self.dpad_down) {
            (true, false) => -1,
            (false, true) => 1,
            _ => 0,
        }
    }

    #[must_use]
    pub fn get(&self, button: GamepadButton) -> bool {
        match button {
            GamepadButton::A => self.a,
            GamepadButton::B => self.b,
            GamepadButton::X => self.x,
            GamepadButton::Y => self.y,
            GamepadButton::Lb => self.lb,
            GamepadButton::Rb => self.rb,
            GamepadButton::L3 => self.ls_click,
            GamepadButton::R3 => self.rs_click,
            GamepadButton::View => self.view,
            GamepadButton::Menu => self.menu,
            GamepadButton::Guide => self.guide,
            GamepadButton::DpadUp => self.dpad_up,
            GamepadButton::DpadDown => self.dpad_down,
            GamepadButton::DpadLeft => self.dpad_left,
            GamepadButton::DpadRight => self.dpad_right,
        }
    }

    pub fn set(&mut self, button: GamepadButton, pressed: bool) {
        match button {
            GamepadButton::A => self.a = pressed,
            GamepadButton::B => self.b = pressed,
            GamepadButton::X => self.x = pressed,
            GamepadButton::Y => self.y = pressed,
            GamepadButton::Lb => self.lb = pressed,
            GamepadButton::Rb => self.rb = pressed,
            GamepadButton::L3 => self.ls_click = pressed,
            GamepadButton::R3 => self.rs_click = pressed,
            GamepadButton::View => self.view = pressed,
            GamepadButton::Menu => self.menu = pressed,
            GamepadButton::Guide => self.guide = pressed,
            GamepadButton::DpadUp => self.dpad_up = pressed,
            GamepadButton::DpadDown => self.dpad_down = pressed,
            GamepadButton::DpadLeft => self.dpad_left = pressed,
            GamepadButton::DpadRight => self.dpad_right = pressed,
        }
    }

    #[must_use]
    pub fn to_bitmask(&self) -> u16 {
        let mut mask = 0u16;
        if self.a {
            mask |= 1 << 0;
        }
        if self.b {
            mask |= 1 << 1;
        }
        if self.x {
            mask |= 1 << 2;
        }
        if self.y {
            mask |= 1 << 3;
        }
        if self.dpad_up {
            mask |= 1 << 4;
        }
        if self.dpad_down {
            mask |= 1 << 5;
        }
        if self.dpad_left {
            mask |= 1 << 6;
        }
        if self.dpad_right {
            mask |= 1 << 7;
        }
        if self.lb {
            mask |= 1 << 8;
        }
        if self.rb {
            mask |= 1 << 9;
        }
        if self.view {
            mask |= 1 << 10;
        }
        if self.menu {
            mask |= 1 << 11;
        }
        if self.guide {
            mask |= 1 << 12;
        }
        if self.ls_click {
            mask |= 1 << 13;
        }
        if self.rs_click {
            mask |= 1 << 14;
        }
        mask
    }
}

/// Independent extra buttons decoded from GameSir vendor HID report `0x12`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraButtons {
    pub l4: bool,
    pub r4: bool,
    pub l5: bool,
    pub r5: bool,
}

impl ExtraButtons {
    #[must_use]
    pub fn to_bitmask(&self) -> u8 {
        let mut mask = 0u8;
        if self.l4 {
            mask |= 1 << 0;
        }
        if self.r4 {
            mask |= 1 << 1;
        }
        if self.l5 {
            mask |= 1 << 2;
        }
        if self.r5 {
            mask |= 1 << 3;
        }
        mask
    }
}

/// Canonical controller state.
///
/// Coordinate convention matches XInput:
/// - `lt`, `rt`: `0..=255` (`0` = released, `255` = full pull)
/// - `lx`, `rx`: `-32768..=32767` (negative = left, positive = right)
/// - `ly`, `ry`: `-32768..=32767` (positive = up, negative = down)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GamepadState {
    pub buttons: GamepadButtons,
    pub extra: ExtraButtons,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

impl GamepadState {
    /// Returns `true` if any standard or extra digital button changed between `self` and `other`.
    #[must_use]
    pub fn buttons_changed(&self, other: &Self) -> bool {
        self.buttons.to_bitmask() != other.buttons.to_bitmask()
            || self.extra.to_bitmask() != other.extra.to_bitmask()
    }

    /// Computes normalized UI percentage values for triggers (`0..=100%`) and sticks (`-100..=100%` + radial magnitude).
    #[must_use]
    pub fn to_normalized_analog(&self) -> NormalizedAnalog {
        let lt_pct = (f32::from(self.lt) / 255.0) * 100.0;
        let rt_pct = (f32::from(self.rt) / 255.0) * 100.0;

        let lx_pct = normalize_axis_i16(self.lx);
        let ly_pct = normalize_axis_i16(self.ly);
        let rx_pct = normalize_axis_i16(self.rx);
        let ry_pct = normalize_axis_i16(self.ry);

        let ls_mag_pct = (lx_pct.hypot(ly_pct)).min(100.0);
        let rs_mag_pct = (rx_pct.hypot(ry_pct)).min(100.0);

        NormalizedAnalog {
            lt_pct: round1(lt_pct),
            rt_pct: round1(rt_pct),
            lx_pct: round1(lx_pct),
            ly_pct: round1(ly_pct),
            rx_pct: round1(rx_pct),
            ry_pct: round1(ry_pct),
            ls_mag_pct: round1(ls_mag_pct),
            rs_mag_pct: round1(rs_mag_pct),
        }
    }
}

/// Force-feedback motor intensities (`0..=255`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RumbleState {
    pub large_motor: u8,
    pub small_motor: u8,
}

/// Live analog percentages for the companion Web UI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct NormalizedAnalog {
    pub lt_pct: f32,
    pub rt_pct: f32,
    pub lx_pct: f32,
    pub ly_pct: f32,
    pub rx_pct: f32,
    pub ry_pct: f32,
    pub ls_mag_pct: f32,
    pub rs_mag_pct: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ActiveSource {
    Gamesir,
    #[default]
    Odin3Builtin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    ConnectedWired,
    ConnectedDongle,
    Quirk10b8,
    #[default]
    Disconnected,
    Paused,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ControllerTelemetry {
    pub buttons: GamepadButtons,
    pub extra: ExtraButtons,
    pub analog: NormalizedAnalog,
}

impl From<&GamepadState> for ControllerTelemetry {
    fn from(state: &GamepadState) -> Self {
        Self {
            buttons: state.buttons,
            extra: state.extra,
            analog: state.to_normalized_analog(),
        }
    }
}

/// Compact WebSocket telemetry frame pushed at 60Hz to the embedded Web UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub active_source: ActiveSource,
    pub connection_state: ConnectionState,
    pub profile: Profile,
    pub usb_hz: u32,
    pub emit_hz: u32,
    pub gamesir: ControllerTelemetry,
    pub odin3: ControllerTelemetry,
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            active_source: ActiveSource::Odin3Builtin,
            connection_state: ConnectionState::Disconnected,
            profile: Profile::Xbox360,
            usb_hz: 0,
            emit_hz: 0,
            gamesir: ControllerTelemetry::default(),
            odin3: ControllerTelemetry::default(),
        }
    }
}

fn normalize_axis_i16(v: i16) -> f32 {
    if v >= 0 {
        (f32::from(v) / 32767.0) * 100.0
    } else {
        (f32::from(v) / 32768.0) * 100.0
    }
}

fn round1(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}
