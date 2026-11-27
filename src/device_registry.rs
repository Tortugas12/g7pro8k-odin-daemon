use serde::{Deserialize, Serialize};

/// GameSir USB Vendor ID.
pub const GAMESIR_VENDOR_ID: u16 = 0x3537;

/// Known wired XInput fallback quirk PID when Linux drops `3537:1034` out of XInput mode.
pub const PID_XINPUT_FALLBACK_QUIRK: u16 = 0x10B8;

/// Recommended Linux kernel quirk parameter for wired `3537:1034`.
pub const KERNEL_QUIRK_PARAM: &str = "usbcore.quirks=3537:1034:r";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    Wired,
    Dongle,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceEdition {
    PlainBlack,
    Nioh3,
    Royal2,
    Championship,
    Aimlabs,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownDevice {
    pub vid: u16,
    pub pid: u16,
    pub edition: DeviceEdition,
    pub connection: ConnectionKind,
    pub name: &'static str,
}

/// Verified GameSir G7 Pro 8K PC USB PIDs (all under VID `0x3537`).
pub const KNOWN_DEVICES: &[KnownDevice] = &[
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x1034,
        edition: DeviceEdition::PlainBlack,
        connection: ConnectionKind::Wired,
        name: "GameSir G7 Pro 8K PC (Plain Black, Wired)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x1035,
        edition: DeviceEdition::PlainBlack,
        connection: ConnectionKind::Dongle,
        name: "GameSir G7 Pro 8K PC (Plain Black, 2.4GHz Dongle)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x10C7,
        edition: DeviceEdition::Nioh3,
        connection: ConnectionKind::Wired,
        name: "GameSir G7 Pro 8K PC (Nioh 3 Edition, Wired)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x10C8,
        edition: DeviceEdition::Nioh3,
        connection: ConnectionKind::Dongle,
        name: "GameSir G7 Pro 8K PC (Nioh 3 Edition, 2.4GHz Dongle)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x1032,
        edition: DeviceEdition::Royal2,
        connection: ConnectionKind::Wired,
        name: "GameSir G7 Pro 8K PC (Royal2 Edition, Wired)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x1033,
        edition: DeviceEdition::Royal2,
        connection: ConnectionKind::Dongle,
        name: "GameSir G7 Pro 8K PC (Royal2 Edition, 2.4GHz Dongle)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x10B7,
        edition: DeviceEdition::Championship,
        connection: ConnectionKind::Wired,
        name: "GameSir G7 Pro 8K PC (Championship Edition, Wired)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x10B9,
        edition: DeviceEdition::Championship,
        connection: ConnectionKind::Dongle,
        name: "GameSir G7 Pro 8K PC (Championship Edition, 2.4GHz Dongle)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x10C5,
        edition: DeviceEdition::Aimlabs,
        connection: ConnectionKind::Wired,
        name: "GameSir G7 Pro 8K PC (Aimlabs Edition, Wired)",
    },
    KnownDevice {
        vid: GAMESIR_VENDOR_ID,
        pid: 0x10C6,
        edition: DeviceEdition::Aimlabs,
        connection: ConnectionKind::Dongle,
        name: "GameSir G7 Pro 8K PC (Aimlabs Edition, 2.4GHz Dongle)",
    },
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchedDevice {
    pub vid: u16,
    pub pid: u16,
    pub edition: DeviceEdition,
    pub connection: ConnectionKind,
    pub name: String,
    pub from_extra_pids: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FallbackQuirkInfo {
    pub vid: u16,
    pub pid: u16,
    pub message: String,
    pub kernel_quirk: String,
    pub recovery_steps: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceMatch {
    Supported(MatchedDevice),
    XInputFallbackQuirk(FallbackQuirkInfo),
    Ignored,
}

impl DeviceMatch {
    #[must_use]
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::Supported(_))
    }

    #[must_use]
    pub fn is_fallback_quirk(&self) -> bool {
        matches!(self, Self::XInputFallbackQuirk(_))
    }
}

/// Strictly classifies a USB VID/PID pair against the GameSir `0x3537` allowlist.
///
/// Safety invariant: ANY device whose Vendor ID is not `0x3537` is unconditionally
/// returned as [`DeviceMatch::Ignored`], regardless of `extra_pids`.
#[must_use]
pub fn classify_usb_device(vid: u16, pid: u16, extra_pids: &[u16]) -> DeviceMatch {
    if vid != GAMESIR_VENDOR_ID {
        return DeviceMatch::Ignored;
    }

    if pid == PID_XINPUT_FALLBACK_QUIRK {
        return DeviceMatch::XInputFallbackQuirk(FallbackQuirkInfo {
            vid,
            pid,
            message: format!(
                "Detected GameSir fallback quirk device {vid:04x}:{pid:04x}. \
                 Wired 3537:1034 dropped out of XInput mode. Hold View + Menu on the controller \
                 for 3 seconds to return to XInput mode, or apply kernel parameter \
                 {KERNEL_QUIRK_PARAM}."
            ),
            kernel_quirk: KERNEL_QUIRK_PARAM.to_string(),
            recovery_steps: vec![
                "Hold View + Menu on the GameSir G7 Pro 8K for ~3 seconds until it re-enumerates in XInput mode (3537:1034).".to_string(),
                "Alternatively, use the 2.4GHz USB dongle (3537:1035), which does not hit the wired fallback quirk.".to_string(),
                format!("On Linux kernels with boot-arg control, add {KERNEL_QUIRK_PARAM}."),
            ],
        });
    }

    if let Some(known) = KNOWN_DEVICES.iter().find(|d| d.pid == pid) {
        return DeviceMatch::Supported(MatchedDevice {
            vid: known.vid,
            pid: known.pid,
            edition: known.edition,
            connection: known.connection,
            name: known.name.to_string(),
            from_extra_pids: false,
        });
    }

    if extra_pids.contains(&pid) {
        return DeviceMatch::Supported(MatchedDevice {
            vid,
            pid,
            edition: DeviceEdition::Custom,
            connection: ConnectionKind::Custom,
            name: format!("GameSir G7 Pro 8K Custom PID ({vid:04x}:{pid:04x})"),
            from_extra_pids: true,
        });
    }

    DeviceMatch::Ignored
}

/// Returns `true` only when the USB device is strictly allowlisted for claiming.
#[must_use]
pub fn is_allowed_device(vid: u16, pid: u16, extra_pids: &[u16]) -> bool {
    classify_usb_device(vid, pid, extra_pids).is_supported()
}
