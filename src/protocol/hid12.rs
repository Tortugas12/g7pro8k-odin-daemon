use crate::config::{BitSpec, Hid12Layout};
use crate::protocol::state::ExtraButtons;

/// Decoded payload from GameSir vendor HID report `0x12`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hid12Frame {
    pub extra: ExtraButtons,
    /// Optional 6-axis raw IMU slice (`[gx, gy, gz, ax, ay, az]`, little-endian `i16`).
    pub gyro_raw: Option<[i16; 6]>,
}

/// Outcome of parsing a vendor HID packet on Interface 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hid12ParseOutcome {
    Parsed(Hid12Frame),
    IgnoredReportId(u8),
    TooShort {
        report_id: u8,
        expected_min: usize,
        actual: usize,
    },
    Empty,
}

/// Pure, allocation-free parser for GameSir vendor HID report `0x12`.
///
/// Gracefully ignores non-matching report IDs (`0x10`, `0x0F`, etc.) and short
/// frames without panicking.
#[must_use]
pub fn parse_hid12_report(buf: &[u8], layout: &Hid12Layout) -> Hid12ParseOutcome {
    let Some(&report_id) = buf.first() else {
        return Hid12ParseOutcome::Empty;
    };

    if report_id != layout.report_id {
        return Hid12ParseOutcome::IgnoredReportId(report_id);
    }

    let required_len = layout
        .min_length
        .max(layout.l4.byte_offset + 1)
        .max(layout.r4.byte_offset + 1)
        .max(layout.l5.byte_offset + 1)
        .max(layout.r5.byte_offset + 1);

    if buf.len() < required_len {
        return Hid12ParseOutcome::TooShort {
            report_id,
            expected_min: required_len,
            actual: buf.len(),
        };
    }

    let extra = ExtraButtons {
        l4: read_bit(buf, layout.l4),
        r4: read_bit(buf, layout.r4),
        l5: read_bit(buf, layout.l5),
        r5: read_bit(buf, layout.r5),
    };

    let gyro_raw = layout.gyro.and_then(|slice| {
        if slice.length != 12 {
            return None;
        }
        let end = slice.byte_offset.checked_add(12)?;
        let bytes = buf.get(slice.byte_offset..end)?;
        Some([
            i16::from_le_bytes([bytes[0], bytes[1]]),
            i16::from_le_bytes([bytes[2], bytes[3]]),
            i16::from_le_bytes([bytes[4], bytes[5]]),
            i16::from_le_bytes([bytes[6], bytes[7]]),
            i16::from_le_bytes([bytes[8], bytes[9]]),
            i16::from_le_bytes([bytes[10], bytes[11]]),
        ])
    });

    Hid12ParseOutcome::Parsed(Hid12Frame { extra, gyro_raw })
}

fn read_bit(buf: &[u8], spec: BitSpec) -> bool {
    buf.get(spec.byte_offset)
        .is_some_and(|&byte| (byte & spec.mask) != 0)
}
