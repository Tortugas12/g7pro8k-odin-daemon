use std::fs::{self, File};
use std::io::Write;
use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

use crate::device_registry::PID_XINPUT_FALLBACK_QUIRK;

pub const DEFAULT_WEB_BIND: &str = "127.0.0.1:8787";
pub const DEFAULT_PID_FILE: &str = "/data/local/tmp/g7pro8k/daemon.pid";
pub const DEFAULT_EMIT_RATE_HZ: u32 = 1000;
pub const MIN_EMIT_RATE_HZ: u32 = 125;
pub const MAX_EMIT_RATE_HZ: u32 = 1000;

/// Supported Linux `KEY_*` names for the companion extra-keys virtual keyboard.
pub const SUPPORTED_KEY_NAMES: &[&str] = &[
    "KEY_F1",
    "KEY_F2",
    "KEY_F3",
    "KEY_F4",
    "KEY_F5",
    "KEY_F6",
    "KEY_F7",
    "KEY_F8",
    "KEY_F9",
    "KEY_F10",
    "KEY_F11",
    "KEY_F12",
    "KEY_TAB",
    "KEY_ESC",
    "KEY_LEFTSHIFT",
    "KEY_RIGHTSHIFT",
    "KEY_LEFTCTRL",
    "KEY_RIGHTCTRL",
    "KEY_LEFTALT",
    "KEY_RIGHTALT",
    "KEY_SPACE",
    "KEY_ENTER",
    "KEY_BACKSPACE",
    "KEY_1",
    "KEY_2",
    "KEY_3",
    "KEY_4",
    "KEY_5",
    "KEY_6",
    "KEY_7",
    "KEY_8",
    "KEY_9",
    "KEY_0",
    "KEY_Q",
    "KEY_E",
    "KEY_R",
    "KEY_F",
    "KEY_G",
    "KEY_I",
    "KEY_M",
    "KEY_C",
    "KEY_V",
    "KEY_X",
    "KEY_Z",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    #[default]
    Xbox360,
    Ds4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum GamepadButton {
    A,
    B,
    X,
    Y,
    #[serde(alias = "LB", alias = "L1")]
    Lb,
    #[serde(alias = "RB", alias = "R1")]
    Rb,
    #[serde(alias = "L3", alias = "LS", alias = "THUMBL")]
    L3,
    #[serde(alias = "R3", alias = "RS", alias = "THUMBR")]
    R3,
    #[serde(alias = "VIEW", alias = "BACK", alias = "SELECT", alias = "SHARE")]
    View,
    #[serde(alias = "MENU", alias = "START", alias = "OPTIONS")]
    Menu,
    #[serde(alias = "GUIDE", alias = "HOME", alias = "XBOX", alias = "PS")]
    Guide,
    #[serde(alias = "DPAD_UP")]
    DpadUp,
    #[serde(alias = "DPAD_DOWN")]
    DpadDown,
    #[serde(alias = "DPAD_LEFT")]
    DpadLeft,
    #[serde(alias = "DPAD_RIGHT")]
    DpadRight,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExtraButtonTarget {
    Key { code: String },
    Gamepad { button: GamepadButton },
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtraButtonsConfig {
    #[serde(default = "default_l4_target")]
    pub l4: ExtraButtonTarget,
    #[serde(default = "default_r4_target")]
    pub r4: ExtraButtonTarget,
    #[serde(default = "default_l5_target")]
    pub l5: ExtraButtonTarget,
    #[serde(default = "default_r5_target")]
    pub r5: ExtraButtonTarget,
}

fn default_l4_target() -> ExtraButtonTarget {
    ExtraButtonTarget::Key {
        code: "KEY_F1".to_string(),
    }
}

fn default_r4_target() -> ExtraButtonTarget {
    ExtraButtonTarget::Key {
        code: "KEY_F2".to_string(),
    }
}

fn default_l5_target() -> ExtraButtonTarget {
    ExtraButtonTarget::Key {
        code: "KEY_F3".to_string(),
    }
}

fn default_r5_target() -> ExtraButtonTarget {
    ExtraButtonTarget::Key {
        code: "KEY_F4".to_string(),
    }
}

impl Default for ExtraButtonsConfig {
    fn default() -> Self {
        Self {
            l4: default_l4_target(),
            r4: default_r4_target(),
            l5: default_l5_target(),
            r5: default_r5_target(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitSpec {
    pub byte_offset: usize,
    pub mask: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GyroSliceSpec {
    pub byte_offset: usize,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hid12Layout {
    #[serde(default = "default_hid12_report_id")]
    pub report_id: u8,
    #[serde(default = "default_hid12_min_length")]
    pub min_length: usize,
    #[serde(default = "default_l4_bit")]
    pub l4: BitSpec,
    #[serde(default = "default_r4_bit")]
    pub r4: BitSpec,
    #[serde(default = "default_l5_bit")]
    pub l5: BitSpec,
    #[serde(default = "default_r5_bit")]
    pub r5: BitSpec,
    #[serde(default = "default_gyro_slice")]
    pub gyro: Option<GyroSliceSpec>,
}

fn default_hid12_report_id() -> u8 {
    0x12
}

fn default_hid12_min_length() -> usize {
    16
}

fn default_l4_bit() -> BitSpec {
    BitSpec {
        byte_offset: 13,
        mask: 0x01,
    }
}

fn default_r4_bit() -> BitSpec {
    BitSpec {
        byte_offset: 13,
        mask: 0x02,
    }
}

fn default_l5_bit() -> BitSpec {
    BitSpec {
        byte_offset: 13,
        mask: 0x04,
    }
}

fn default_r5_bit() -> BitSpec {
    BitSpec {
        byte_offset: 13,
        mask: 0x08,
    }
}

fn default_gyro_slice() -> Option<GyroSliceSpec> {
    Some(GyroSliceSpec {
        byte_offset: 16,
        length: 12,
    })
}

impl Default for Hid12Layout {
    fn default() -> Self {
        Self {
            report_id: default_hid12_report_id(),
            min_length: default_hid12_min_length(),
            l4: default_l4_bit(),
            r4: default_r4_bit(),
            l5: default_l5_bit(),
            r5: default_r5_bit(),
            gyro: default_gyro_slice(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebUiConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_bind")]
    pub bind: String,
}

fn default_true() -> bool {
    true
}

fn default_web_bind() -> String {
    DEFAULT_WEB_BIND.to_string()
}

fn default_emit_rate_hz() -> u32 {
    DEFAULT_EMIT_RATE_HZ
}

fn default_pid_file() -> String {
    DEFAULT_PID_FILE.to_string()
}

impl Default for WebUiConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bind: default_web_bind(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub profile: Profile,
    #[serde(default = "default_emit_rate_hz")]
    pub emit_rate_hz: u32,
    #[serde(default, deserialize_with = "deserialize_pid_list")]
    pub extra_pids: Vec<u16>,
    #[serde(default = "default_true")]
    pub enable_rumble: bool,
    #[serde(default)]
    pub extra_buttons: ExtraButtonsConfig,
    #[serde(default)]
    pub hid12_layout: Hid12Layout,
    #[serde(default)]
    pub web_ui: WebUiConfig,
    #[serde(default = "default_pid_file")]
    pub pid_file: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: Profile::Xbox360,
            emit_rate_hz: DEFAULT_EMIT_RATE_HZ,
            extra_pids: Vec::new(),
            enable_rumble: true,
            extra_buttons: ExtraButtonsConfig::default(),
            hid12_layout: Hid12Layout::default(),
            web_ui: WebUiConfig::default(),
            pid_file: default_pid_file(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("configuration validation failed ({} error(s))", .errors.len())]
pub struct ConfigValidationError {
    pub errors: Vec<FieldError>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read or write config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Validation(#[from] ConfigValidationError),
}

impl Config {
    /// Validates every field in the configuration and collects structured field-level errors.
    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        let mut errors = Vec::new();

        if self.emit_rate_hz != 0
            && !(MIN_EMIT_RATE_HZ..=MAX_EMIT_RATE_HZ).contains(&self.emit_rate_hz)
        {
            errors.push(FieldError {
                field: "emit_rate_hz".to_string(),
                message: format!(
                    "must be 0 (uncapped) or between {MIN_EMIT_RATE_HZ} and {MAX_EMIT_RATE_HZ}, got {}",
                    self.emit_rate_hz
                ),
            });
        }

        for (idx, &pid) in self.extra_pids.iter().enumerate() {
            if pid == 0x0000 || pid == 0xFFFF {
                errors.push(FieldError {
                    field: format!("extra_pids[{idx}]"),
                    message: format!("invalid USB PID 0x{pid:04X}"),
                });
            } else if pid == PID_XINPUT_FALLBACK_QUIRK {
                errors.push(FieldError {
                    field: format!("extra_pids[{idx}]"),
                    message: "0x10B8 is the wired XInput fallback quirk PID and cannot be claimed as an XInput device; hold View + Menu on the controller instead".to_string(),
                });
            }
        }

        validate_extra_button("extra_buttons.l4", &self.extra_buttons.l4, &mut errors);
        validate_extra_button("extra_buttons.r4", &self.extra_buttons.r4, &mut errors);
        validate_extra_button("extra_buttons.l5", &self.extra_buttons.l5, &mut errors);
        validate_extra_button("extra_buttons.r5", &self.extra_buttons.r5, &mut errors);

        validate_hid12_layout(&self.hid12_layout, &mut errors);

        match self.web_ui.bind.parse::<SocketAddr>() {
            Ok(addr) => {
                if !addr.ip().is_loopback() {
                    errors.push(FieldError {
                        field: "web_ui.bind".to_string(),
                        message: format!(
                            "must bind to a loopback address (e.g. 127.0.0.1:8787), refused non-loopback address {}",
                            addr.ip()
                        ),
                    });
                }
            }
            Err(err) => {
                errors.push(FieldError {
                    field: "web_ui.bind".to_string(),
                    message: format!("invalid socket address '{}': {err}", self.web_ui.bind),
                });
            }
        }

        if self.pid_file.trim().is_empty() {
            errors.push(FieldError {
                field: "pid_file".to_string(),
                message: "must not be empty".to_string(),
            });
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ConfigValidationError { errors })
        }
    }

    /// Loads and validates a configuration file from disk.
    pub fn load_from_file(path: &Path) -> Result<Self, ConfigError> {
        let raw = fs::read_to_string(path)?;
        let cfg: Self = serde_json::from_str(&raw)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validates, writes to a sibling temporary file, fsyncs, and atomically renames over `path`.
    pub fn save_atomic(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }

        let payload = serde_json::to_string_pretty(self)?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.json");
        let tmp_name = format!(".{file_name}.tmp.{}", std::process::id());
        let tmp_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
            _ => Path::new(&tmp_name).to_path_buf(),
        };

        let write_result = (|| -> Result<(), std::io::Error> {
            let mut file = File::create(&tmp_path)?;
            file.write_all(payload.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            Ok(())
        })();

        if let Err(err) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(ConfigError::Io(err));
        }

        if let Err(err) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            return Err(ConfigError::Io(err));
        }

        Ok(())
    }
}

fn validate_extra_button(field: &str, target: &ExtraButtonTarget, errors: &mut Vec<FieldError>) {
    if let ExtraButtonTarget::Key { code } = target {
        if !SUPPORTED_KEY_NAMES.contains(&code.as_str()) {
            errors.push(FieldError {
                field: format!("{field}.code"),
                message: format!("unsupported Linux key code '{code}'"),
            });
        }
    }
}

fn validate_hid12_layout(layout: &Hid12Layout, errors: &mut Vec<FieldError>) {
    if layout.min_length < 2 || layout.min_length > 64 {
        errors.push(FieldError {
            field: "hid12_layout.min_length".to_string(),
            message: format!("must be between 2 and 64 bytes, got {}", layout.min_length),
        });
    }

    for (name, spec) in [
        ("hid12_layout.l4", layout.l4),
        ("hid12_layout.r4", layout.r4),
        ("hid12_layout.l5", layout.l5),
        ("hid12_layout.r5", layout.r5),
    ] {
        if spec.byte_offset == 0 || spec.byte_offset >= 64 {
            errors.push(FieldError {
                field: format!("{name}.byte_offset"),
                message: format!(
                    "must be between 1 and 63 (byte 0 is report_id), got {}",
                    spec.byte_offset
                ),
            });
        } else if spec.byte_offset >= layout.min_length {
            errors.push(FieldError {
                field: format!("{name}.byte_offset"),
                message: format!(
                    "byte_offset {} must be less than min_length {}",
                    spec.byte_offset, layout.min_length
                ),
            });
        }

        if spec.mask == 0 {
            errors.push(FieldError {
                field: format!("{name}.mask"),
                message: "bitmask must be non-zero".to_string(),
            });
        }
    }

    if let Some(gyro) = layout.gyro {
        if gyro.byte_offset == 0 || gyro.length != 12 || gyro.byte_offset + gyro.length > 64 {
            errors.push(FieldError {
                field: "hid12_layout.gyro".to_string(),
                message: format!(
                    "gyro slice must start at byte >= 1 with length 12 and fit within 64 bytes (got offset={}, length={})",
                    gyro.byte_offset, gyro.length
                ),
            });
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PidInput {
    Number(u16),
    HexString(String),
}

fn deserialize_pid_list<'de, D>(deserializer: D) -> Result<Vec<u16>, D::Error>
where
    D: Deserializer<'de>,
{
    let items = Vec::<PidInput>::deserialize(deserializer)?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item {
            PidInput::Number(v) => out.push(v),
            PidInput::HexString(s) => {
                let trimmed = s.trim();
                let parsed = if let Some(hex) = trimmed
                    .strip_prefix("0x")
                    .or_else(|| trimmed.strip_prefix("0X"))
                {
                    u16::from_str_radix(hex, 16)
                } else {
                    u16::from_str_radix(trimmed, 16)
                };
                match parsed {
                    Ok(v) => out.push(v),
                    Err(_) => {
                        return Err(serde::de::Error::custom(format!(
                            "invalid hex USB PID '{s}'"
                        )))
                    }
                }
            }
        }
    }
    Ok(out)
}
