use thiserror::Error;

use crate::protocol::state::{GamepadButtons, GamepadState, RumbleState};

/// Standard XInput interrupt IN report length in bytes.
pub const XINPUT_IN_REPORT_LEN: usize = 20;

/// Standard XInput rumble interrupt OUT report length in bytes.
pub const XINPUT_OUT_RUMBLE_LEN: usize = 8;

/// Standard XInput input report header bytes (`msg_type = 0x00`, `len = 0x14`).
pub const XINPUT_IN_MSG_TYPE: u8 = 0x00;
pub const XINPUT_IN_PAYLOAD_LEN: u8 = 0x14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XInputReport {
    pub buttons: GamepadButtons,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

impl XInputReport {
    /// Applies the decoded XInput standard inputs onto `state`, preserving any `extra` buttons.
    pub fn apply_to(&self, state: &mut GamepadState) {
        state.buttons = self.buttons;
        state.lt = self.lt;
        state.rt = self.rt;
        state.lx = self.lx;
        state.ly = self.ly;
        state.rx = self.rx;
        state.ry = self.ry;
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum XInputError {
    #[error("XInput report too short: expected at least {expected} bytes, got {actual}")]
    TooShort { expected: usize, actual: usize },
    #[error("unexpected XInput header: type=0x{msg_type:02X}, len=0x{payload_len:02X}")]
    InvalidHeader { msg_type: u8, payload_len: u8 },
}

/// Pure, allocation-free parser for standard 20-byte XInput IN reports.
pub fn parse_xinput_report(buf: &[u8]) -> Result<XInputReport, XInputError> {
    if buf.len() < XINPUT_IN_REPORT_LEN {
        return Err(XInputError::TooShort {
            expected: XINPUT_IN_REPORT_LEN,
            actual: buf.len(),
        });
    }

    if buf[0] != XINPUT_IN_MSG_TYPE || buf[1] != XINPUT_IN_PAYLOAD_LEN {
        return Err(XInputError::InvalidHeader {
            msg_type: buf[0],
            payload_len: buf[1],
        });
    }

    let lo = buf[2];
    let hi = buf[3];

    let buttons = GamepadButtons {
        dpad_up: (lo & 0x01) != 0,
        dpad_down: (lo & 0x02) != 0,
        dpad_left: (lo & 0x04) != 0,
        dpad_right: (lo & 0x08) != 0,
        menu: (lo & 0x10) != 0,
        view: (lo & 0x20) != 0,
        ls_click: (lo & 0x40) != 0,
        rs_click: (lo & 0x80) != 0,
        lb: (hi & 0x01) != 0,
        rb: (hi & 0x02) != 0,
        guide: (hi & 0x04) != 0,
        a: (hi & 0x10) != 0,
        b: (hi & 0x20) != 0,
        x: (hi & 0x40) != 0,
        y: (hi & 0x80) != 0,
    };

    let lt = buf[4];
    let rt = buf[5];
    let lx = i16::from_le_bytes([buf[6], buf[7]]);
    let ly = i16::from_le_bytes([buf[8], buf[9]]);
    let rx = i16::from_le_bytes([buf[10], buf[11]]);
    let ry = i16::from_le_bytes([buf[12], buf[13]]);

    Ok(XInputReport {
        buttons,
        lt,
        rt,
        lx,
        ly,
        rx,
        ry,
    })
}

/// Encodes a standard 8-byte XInput rumble output report.
///
/// Safety invariant: this is the ONLY USB OUT payload the daemon ever transmits.
#[must_use]
pub fn encode_rumble_report(rumble: RumbleState) -> [u8; XINPUT_OUT_RUMBLE_LEN] {
    [
        0x00,
        0x08,
        0x00,
        rumble.large_motor,
        rumble.small_motor,
        0x00,
        0x00,
        0x00,
    ]
}
