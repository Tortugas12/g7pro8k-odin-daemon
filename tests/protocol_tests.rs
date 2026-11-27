use std::path::Path;

use g7pro8k_odin_daemon::config::{
    BitSpec, Config, ExtraButtonTarget, GamepadButton, GyroSliceSpec, Hid12Layout, Profile,
};
use g7pro8k_odin_daemon::device_registry::{
    classify_usb_device, is_allowed_device, ConnectionKind, DeviceEdition, DeviceMatch,
    GAMESIR_VENDOR_ID, KERNEL_QUIRK_PARAM, KNOWN_DEVICES, PID_XINPUT_FALLBACK_QUIRK,
};
use g7pro8k_odin_daemon::protocol::hid12::{parse_hid12_report, Hid12ParseOutcome};
use g7pro8k_odin_daemon::protocol::state::{ExtraButtons, GamepadState, RumbleState};
use g7pro8k_odin_daemon::protocol::xinput::{
    encode_rumble_report, parse_xinput_report, XInputError,
};
use tempfile::tempdir;

#[derive(Default)]
struct XInputTestInput {
    lo: u8,
    hi: u8,
    lt: u8,
    rt: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

fn make_xinput_packet(input: XInputTestInput) -> [u8; 20] {
    let mut pkt = [0u8; 20];
    pkt[0] = 0x00;
    pkt[1] = 0x14;
    pkt[2] = input.lo;
    pkt[3] = input.hi;
    pkt[4] = input.lt;
    pkt[5] = input.rt;
    pkt[6..8].copy_from_slice(&input.lx.to_le_bytes());
    pkt[8..10].copy_from_slice(&input.ly.to_le_bytes());
    pkt[10..12].copy_from_slice(&input.rx.to_le_bytes());
    pkt[12..14].copy_from_slice(&input.ry.to_le_bytes());
    pkt
}

#[test]
fn xinput_decodes_all_buttons_and_extremes() {
    let pkt = make_xinput_packet(XInputTestInput {
        lo: 0xFF,
        hi: 0xF7,
        lt: 255,
        rt: 128,
        lx: -32768,
        ly: 32767,
        rx: 16384,
        ry: -16384,
    });
    let report = parse_xinput_report(&pkt).expect("valid 20-byte XInput report");

    assert!(report.buttons.dpad_up);
    assert!(report.buttons.dpad_down);
    assert!(report.buttons.dpad_left);
    assert!(report.buttons.dpad_right);
    assert!(report.buttons.menu);
    assert!(report.buttons.view);
    assert!(report.buttons.ls_click);
    assert!(report.buttons.rs_click);
    assert!(report.buttons.lb);
    assert!(report.buttons.rb);
    assert!(report.buttons.guide);
    assert!(report.buttons.a);
    assert!(report.buttons.b);
    assert!(report.buttons.x);
    assert!(report.buttons.y);
    assert_eq!(report.lt, 255);
    assert_eq!(report.rt, 128);
    assert_eq!(report.lx, -32768);
    assert_eq!(report.ly, 32767);
    assert_eq!(report.rx, 16384);
    assert_eq!(report.ry, -16384);
}

#[test]
fn xinput_decodes_each_individual_button_bit() {
    let lo_cases: &[(u8, GamepadButton)] = &[
        (0x01, GamepadButton::DpadUp),
        (0x02, GamepadButton::DpadDown),
        (0x04, GamepadButton::DpadLeft),
        (0x08, GamepadButton::DpadRight),
        (0x10, GamepadButton::Menu),
        (0x20, GamepadButton::View),
        (0x40, GamepadButton::L3),
        (0x80, GamepadButton::R3),
    ];

    for &(mask, btn) in lo_cases {
        let pkt = make_xinput_packet(XInputTestInput {
            lo: mask,
            ..Default::default()
        });
        let report = parse_xinput_report(&pkt).unwrap();
        assert!(
            report.buttons.get(btn),
            "expected {btn:?} set for lo=0x{mask:02X}"
        );
        assert_eq!(report.buttons.to_bitmask().count_ones(), 1);
    }

    let hi_cases: &[(u8, GamepadButton)] = &[
        (0x01, GamepadButton::Lb),
        (0x02, GamepadButton::Rb),
        (0x04, GamepadButton::Guide),
        (0x10, GamepadButton::A),
        (0x20, GamepadButton::B),
        (0x40, GamepadButton::X),
        (0x80, GamepadButton::Y),
    ];

    for &(mask, btn) in hi_cases {
        let pkt = make_xinput_packet(XInputTestInput {
            hi: mask,
            ..Default::default()
        });
        let report = parse_xinput_report(&pkt).unwrap();
        assert!(
            report.buttons.get(btn),
            "expected {btn:?} set for hi=0x{mask:02X}"
        );
        assert_eq!(report.buttons.to_bitmask().count_ones(), 1);
    }
}

#[test]
fn xinput_rejects_short_and_invalid_headers() {
    assert_eq!(
        parse_xinput_report(&[0x00, 0x14, 0x00]),
        Err(XInputError::TooShort {
            expected: 20,
            actual: 3
        })
    );

    let mut bad_type = [0u8; 20];
    bad_type[0] = 0x01;
    bad_type[1] = 0x14;
    assert_eq!(
        parse_xinput_report(&bad_type),
        Err(XInputError::InvalidHeader {
            msg_type: 0x01,
            payload_len: 0x14
        })
    );

    let mut bad_len = [0u8; 20];
    bad_len[0] = 0x00;
    bad_len[1] = 0x08;
    assert_eq!(
        parse_xinput_report(&bad_len),
        Err(XInputError::InvalidHeader {
            msg_type: 0x00,
            payload_len: 0x08
        })
    );
}

#[test]
fn xinput_apply_preserves_extra_buttons() {
    let mut state = GamepadState {
        extra: ExtraButtons {
            l4: true,
            r4: false,
            l5: true,
            r5: false,
        },
        ..Default::default()
    };
    let pkt = make_xinput_packet(XInputTestInput {
        hi: 0x10,
        lt: 200,
        rt: 50,
        lx: 1000,
        ly: -1000,
        ..Default::default()
    });
    let report = parse_xinput_report(&pkt).unwrap();
    report.apply_to(&mut state);

    assert!(state.buttons.a);
    assert_eq!(state.lt, 200);
    assert_eq!(state.rt, 50);
    assert!(state.extra.l4);
    assert!(state.extra.l5);
    assert!(!state.extra.r4);
}

#[test]
fn xinput_rumble_encoder_matches_wire_format() {
    assert_eq!(
        encode_rumble_report(RumbleState {
            large_motor: 0,
            small_motor: 0
        }),
        [0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]
    );
    assert_eq!(
        encode_rumble_report(RumbleState {
            large_motor: 0xC0,
            small_motor: 0x7F
        }),
        [0x00, 0x08, 0x00, 0xC0, 0x7F, 0x00, 0x00, 0x00]
    );
}

#[test]
fn hid12_decodes_default_and_custom_layouts_safely() {
    let layout = Hid12Layout::default();

    // Non-0x12 report IDs are gracefully ignored.
    assert_eq!(
        parse_hid12_report(&[0x10, 0x01, 0x02], &layout),
        Hid12ParseOutcome::IgnoredReportId(0x10)
    );
    assert_eq!(
        parse_hid12_report(&[0x0F, 0x00], &layout),
        Hid12ParseOutcome::IgnoredReportId(0x0F)
    );
    assert_eq!(parse_hid12_report(&[], &layout), Hid12ParseOutcome::Empty);

    // Short 0x12 frame returns TooShort without panicking.
    assert_eq!(
        parse_hid12_report(&[0x12, 0x00, 0x00], &layout),
        Hid12ParseOutcome::TooShort {
            report_id: 0x12,
            expected_min: 16,
            actual: 3
        }
    );

    // Valid 16-byte frame decodes extra buttons even without gyro tail.
    let mut pkt16 = [0u8; 16];
    pkt16[0] = 0x12;
    pkt16[13] = 0x05; // l4 + l5
    match parse_hid12_report(&pkt16, &layout) {
        Hid12ParseOutcome::Parsed(frame) => {
            assert_eq!(
                frame.extra,
                ExtraButtons {
                    l4: true,
                    r4: false,
                    l5: true,
                    r5: false
                }
            );
            assert!(frame.gyro_raw.is_none());
        }
        other => panic!("expected Parsed, got {other:?}"),
    }

    // Valid 32-byte frame also decodes gyro slice.
    let mut pkt32 = [0u8; 32];
    pkt32[0] = 0x12;
    pkt32[13] = 0x0A; // r4 + r5
    let expected_imu: [i16; 6] = [100, -200, 300, 4096, -4096, 8192];
    for (i, val) in expected_imu.iter().enumerate() {
        pkt32[16 + i * 2..18 + i * 2].copy_from_slice(&val.to_le_bytes());
    }
    match parse_hid12_report(&pkt32, &layout) {
        Hid12ParseOutcome::Parsed(frame) => {
            assert_eq!(
                frame.extra,
                ExtraButtons {
                    l4: false,
                    r4: true,
                    l5: false,
                    r5: true
                }
            );
            assert_eq!(frame.gyro_raw, Some(expected_imu));
        }
        other => panic!("expected Parsed, got {other:?}"),
    }

    // Custom Day-1 overridden offsets.
    let custom = Hid12Layout {
        report_id: 0x12,
        min_length: 20,
        l4: BitSpec {
            byte_offset: 18,
            mask: 0x10,
        },
        r4: BitSpec {
            byte_offset: 18,
            mask: 0x20,
        },
        l5: BitSpec {
            byte_offset: 19,
            mask: 0x40,
        },
        r5: BitSpec {
            byte_offset: 19,
            mask: 0x80,
        },
        gyro: Some(GyroSliceSpec {
            byte_offset: 20,
            length: 12,
        }),
    };
    let mut custom_pkt = [0u8; 20];
    custom_pkt[0] = 0x12;
    custom_pkt[18] = 0x20;
    custom_pkt[19] = 0xC0;
    match parse_hid12_report(&custom_pkt, &custom) {
        Hid12ParseOutcome::Parsed(frame) => {
            assert_eq!(
                frame.extra,
                ExtraButtons {
                    l4: false,
                    r4: true,
                    l5: true,
                    r5: true
                }
            );
        }
        other => panic!("expected Parsed, got {other:?}"),
    }
}

#[test]
fn normalized_analog_percentages_and_clamping() {
    let state = GamepadState {
        lt: 255,
        rt: 0,
        lx: 32767,
        ly: 32767,
        rx: -32768,
        ry: 0,
        ..Default::default()
    };
    let norm = state.to_normalized_analog();
    assert_eq!(norm.lt_pct, 100.0);
    assert_eq!(norm.rt_pct, 0.0);
    assert_eq!(norm.lx_pct, 100.0);
    assert_eq!(norm.ly_pct, 100.0);
    assert_eq!(norm.ls_mag_pct, 100.0);
    assert_eq!(norm.rx_pct, -100.0);
    assert_eq!(norm.ry_pct, 0.0);
    assert_eq!(norm.rs_mag_pct, 100.0);
}

#[test]
fn device_registry_verifies_allowlist_and_10b8_quirk() {
    assert_eq!(KNOWN_DEVICES.len(), 10);

    // Primary plain black targets.
    match classify_usb_device(GAMESIR_VENDOR_ID, 0x1034, &[]) {
        DeviceMatch::Supported(m) => {
            assert_eq!(m.edition, DeviceEdition::PlainBlack);
            assert_eq!(m.connection, ConnectionKind::Wired);
            assert!(!m.from_extra_pids);
        }
        other => panic!("expected Supported for 3537:1034, got {other:?}"),
    }

    match classify_usb_device(GAMESIR_VENDOR_ID, 0x1035, &[]) {
        DeviceMatch::Supported(m) => {
            assert_eq!(m.edition, DeviceEdition::PlainBlack);
            assert_eq!(m.connection, ConnectionKind::Dongle);
        }
        other => panic!("expected Supported for 3537:1035, got {other:?}"),
    }

    // Fallback quirk 3537:10b8.
    match classify_usb_device(GAMESIR_VENDOR_ID, PID_XINPUT_FALLBACK_QUIRK, &[]) {
        DeviceMatch::XInputFallbackQuirk(info) => {
            assert!(info.message.contains("View + Menu"));
            assert!(info.message.contains(KERNEL_QUIRK_PARAM));
            assert_eq!(info.kernel_quirk, KERNEL_QUIRK_PARAM);
        }
        other => panic!("expected XInputFallbackQuirk, got {other:?}"),
    }

    // Extra PID works ONLY for GameSir VID 0x3537.
    assert!(is_allowed_device(GAMESIR_VENDOR_ID, 0x10FF, &[0x10FF]));
    assert!(!is_allowed_device(0x045E, 0x10FF, &[0x10FF]));
    assert!(!is_allowed_device(0x045E, 0x028E, &[]));
    assert!(!is_allowed_device(0x054C, 0x09CC, &[]));
}

#[test]
fn example_config_loads_and_atomic_save_roundtrips() {
    let example_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.json");
    let cfg = Config::load_from_file(&example_path).expect("config.example.json should be valid");
    assert_eq!(cfg.profile, Profile::Xbox360);
    assert_eq!(cfg.emit_rate_hz, 1000);
    assert!(cfg.enable_rumble);

    let dir = tempdir().unwrap();
    let dest = dir.path().join("nested").join("config.json");
    let mut edited = cfg.clone();
    edited.profile = Profile::Ds4;
    edited.emit_rate_hz = 500;
    edited.extra_buttons.l4 = ExtraButtonTarget::Gamepad {
        button: GamepadButton::L3,
    };
    edited.save_atomic(&dest).expect("atomic save succeeds");

    let reloaded = Config::load_from_file(&dest).expect("reloaded config valid");
    assert_eq!(reloaded, edited);
}

#[test]
fn config_rejects_invalid_values_and_accepts_hex_extra_pids() {
    let json = r#"{
        "profile": "xbox360",
        "emit_rate_hz": 1000,
        "extra_pids": ["0x1099", 4250]
    }"#;
    let parsed: Config = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.extra_pids, vec![0x1099, 4250]);
    parsed.validate().unwrap();

    let mut invalid = Config {
        emit_rate_hz: 50,
        extra_pids: vec![PID_XINPUT_FALLBACK_QUIRK],
        ..Default::default()
    };
    invalid.web_ui.bind = "0.0.0.0:8787".to_string();
    invalid.extra_buttons.l4 = ExtraButtonTarget::Key {
        code: "KEY_NOT_REAL".to_string(),
    };

    let err = invalid.validate().expect_err("must fail validation");
    let fields: Vec<&str> = err.errors.iter().map(|e| e.field.as_str()).collect();
    assert!(fields.contains(&"emit_rate_hz"));
    assert!(fields.contains(&"web_ui.bind"));
    assert!(fields.contains(&"extra_buttons.l4.code"));
    assert!(fields.contains(&"extra_pids[0]"));
}
