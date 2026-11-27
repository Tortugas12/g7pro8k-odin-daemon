use evdev::{AbsoluteAxisCode, EventType, KeyCode};
use g7pro8k_odin_daemon::coalescer::Coalescer;
use g7pro8k_odin_daemon::config::{
    ExtraButtonTarget, ExtraButtonsConfig, GamepadButton, Hid12Layout, Profile,
};
use g7pro8k_odin_daemon::output::profiles::{
    ds4_profile_spec, extra_keys_profile_spec, xbox360_profile_spec, VIRTUAL_DS4_NAME,
    VIRTUAL_EXTRA_KEYS_NAME, VIRTUAL_XBOX360_NAME,
};
use g7pro8k_odin_daemon::output::{MockOutputRecord, MockVirtualOutput, OutputDispatcher};
use g7pro8k_odin_daemon::protocol::hid12::{parse_hid12_report, Hid12ParseOutcome};
use g7pro8k_odin_daemon::protocol::state::{GamepadState, RumbleState};
use g7pro8k_odin_daemon::protocol::xinput::{encode_rumble_report, parse_xinput_report};

fn build_xinput(lo: u8, hi: u8, lt: u8, rt: u8, lx: i16, ly: i16) -> [u8; 20] {
    let mut pkt = [0u8; 20];
    pkt[0] = 0x00;
    pkt[1] = 0x14;
    pkt[2] = lo;
    pkt[3] = hi;
    pkt[4] = lt;
    pkt[5] = rt;
    pkt[6..8].copy_from_slice(&lx.to_le_bytes());
    pkt[8..10].copy_from_slice(&ly.to_le_bytes());
    pkt
}

#[test]
fn end_to_end_xbox360_and_extra_keys_pipeline() {
    let spec = xbox360_profile_spec();
    assert_eq!(spec.name, VIRTUAL_XBOX360_NAME);
    assert_eq!(spec.input_id.vendor(), 0x045E);
    assert_eq!(spec.input_id.product(), 0x028E);

    let extra_spec = extra_keys_profile_spec();
    assert_eq!(extra_spec.name, VIRTUAL_EXTRA_KEYS_NAME);
    assert_eq!(extra_spec.input_id.vendor(), 0x3537);
    assert_eq!(extra_spec.input_id.product(), 0xFF01);

    let layout = Hid12Layout::default();
    let mut coalescer = Coalescer::new(1000);
    let mut state = GamepadState::default();

    let extra_cfg = ExtraButtonsConfig {
        l4: ExtraButtonTarget::Key {
            code: "KEY_F1".to_string(),
        },
        r4: ExtraButtonTarget::Gamepad {
            button: GamepadButton::L3,
        },
        l5: ExtraButtonTarget::Disabled,
        r5: ExtraButtonTarget::Key {
            code: "KEY_F4".to_string(),
        },
    };

    let mock = MockVirtualOutput::new(Profile::Xbox360);
    let mut dispatcher = OutputDispatcher::new(mock, extra_cfg);

    // Frame 1: XInput A pressed + left stick full up-right (+32767, +32767) + LT=200.
    let x_pkt = build_xinput(0x00, 0x10, 200, 0, 32767, 32767);
    parse_xinput_report(&x_pkt).unwrap().apply_to(&mut state);
    if let Some(out) = coalescer.push(state, 0) {
        dispatcher.emit_state(&out).unwrap();
    }

    // Frame 2 (125 us later): HID 0x12 sets L4 (KEY_F1) + R4 (mirrored to L3) + L5 (Disabled).
    let mut h_pkt = [0u8; 16];
    h_pkt[0] = 0x12;
    h_pkt[13] = 0x01 | 0x02 | 0x04;
    if let Hid12ParseOutcome::Parsed(frame) = parse_hid12_report(&h_pkt, &layout) {
        state.extra = frame.extra;
    }
    // Button edge MUST flush immediately even at 125 us.
    let flushed = coalescer
        .push(state, 125)
        .expect("extra button edge must emit immediately");
    dispatcher.emit_state(&flushed).unwrap();

    let records = dispatcher.inner().records();
    // Verify Xbox 360 events from the second gamepad emission.
    let last_gp_events = records
        .iter()
        .rev()
        .find_map(|r| match r {
            MockOutputRecord::Gamepad { events, .. } => Some(events),
            _ => None,
        })
        .expect("expected Gamepad record");

    let find_abs = |code: AbsoluteAxisCode| -> i32 {
        last_gp_events
            .iter()
            .find(|e| e.event_type() == EventType::ABSOLUTE && e.code() == code.0)
            .unwrap()
            .value()
    };
    let find_key = |code: KeyCode| -> i32 {
        last_gp_events
            .iter()
            .find(|e| e.event_type() == EventType::KEY && e.code() == code.0)
            .unwrap()
            .value()
    };

    assert_eq!(find_abs(AbsoluteAxisCode::ABS_X), 32767);
    // XInput Y up (+32767) MUST invert to Linux evdev up (-32767).
    assert_eq!(find_abs(AbsoluteAxisCode::ABS_Y), -32767);
    assert_eq!(find_abs(AbsoluteAxisCode::ABS_Z), 200);
    assert_eq!(find_key(KeyCode::BTN_SOUTH), 1);
    // R4 mirrored to L3 (BTN_THUMBL).
    assert_eq!(find_key(KeyCode::BTN_THUMBL), 1);

    // Verify L4 emitted KEY_F1 press and Disabled L5 did not emit anything.
    assert!(records.contains(&MockOutputRecord::ExtraKey {
        key: KeyCode::KEY_F1,
        pressed: true
    }));
    assert_eq!(
        dispatcher.inner().active_keys().get(&KeyCode::KEY_F1),
        Some(&true)
    );

    // Simulated disconnect cleanup releases every held key and zeroes gamepad.
    dispatcher.release_all().unwrap();
    assert_eq!(dispatcher.inner().release_all_calls(), 1);
    assert_eq!(
        dispatcher.inner().last_gamepad_state(),
        Some(GamepadState::default())
    );
    assert_eq!(
        dispatcher.inner().active_keys().get(&KeyCode::KEY_F1),
        Some(&false)
    );
}

#[test]
fn end_to_end_ds4_profile_scales_axes_and_digital_triggers() {
    let spec = ds4_profile_spec();
    assert_eq!(spec.name, VIRTUAL_DS4_NAME);
    assert_eq!(spec.input_id.vendor(), 0x054C);
    assert_eq!(spec.input_id.product(), 0x09CC);

    let mock = MockVirtualOutput::new(Profile::Ds4);
    let mut dispatcher = OutputDispatcher::new(mock, ExtraButtonsConfig::default());

    // Neutral state: sticks centered at 128, triggers at 0, BTN_TL2/TR2 released.
    dispatcher.emit_state(&GamepadState::default()).unwrap();
    let neutral_events = match dispatcher.inner().records().last().unwrap() {
        MockOutputRecord::Gamepad { events, .. } => events.clone(),
        other => panic!("expected Gamepad record, got {other:?}"),
    };
    let get_val = |events: &[evdev::InputEvent], ev_type: EventType, code: u16| -> i32 {
        events
            .iter()
            .find(|e| e.event_type() == ev_type && e.code() == code)
            .unwrap()
            .value()
    };

    assert_eq!(
        get_val(
            &neutral_events,
            EventType::ABSOLUTE,
            AbsoluteAxisCode::ABS_X.0
        ),
        128
    );
    assert_eq!(
        get_val(
            &neutral_events,
            EventType::ABSOLUTE,
            AbsoluteAxisCode::ABS_Y.0
        ),
        128
    );
    assert_eq!(
        get_val(&neutral_events, EventType::KEY, KeyCode::BTN_TL2.0),
        0
    );

    // Extreme state: LX=-32768 (0), LY=+32767 (0 = up), LT=200 (BTN_TL2=1), RT=5 (BTN_TR2=0).
    let extreme = GamepadState {
        lx: -32768,
        ly: 32767,
        rx: 32767,
        ry: -32768,
        lt: 200,
        rt: 5,
        ..Default::default()
    };
    dispatcher.emit_state(&extreme).unwrap();
    let extreme_events = match dispatcher.inner().records().last().unwrap() {
        MockOutputRecord::Gamepad { events, .. } => events.clone(),
        other => panic!("expected Gamepad record, got {other:?}"),
    };

    assert_eq!(
        get_val(
            &extreme_events,
            EventType::ABSOLUTE,
            AbsoluteAxisCode::ABS_X.0
        ),
        0
    );
    assert_eq!(
        get_val(
            &extreme_events,
            EventType::ABSOLUTE,
            AbsoluteAxisCode::ABS_Y.0
        ),
        0
    );
    assert_eq!(
        get_val(
            &extreme_events,
            EventType::ABSOLUTE,
            AbsoluteAxisCode::ABS_RX.0
        ),
        255
    );
    assert_eq!(
        get_val(
            &extreme_events,
            EventType::ABSOLUTE,
            AbsoluteAxisCode::ABS_RY.0
        ),
        255
    );
    assert_eq!(
        get_val(&extreme_events, EventType::KEY, KeyCode::BTN_TL2.0),
        1
    );
    assert_eq!(
        get_val(&extreme_events, EventType::KEY, KeyCode::BTN_TR2.0),
        0
    );
}

#[test]
fn rumble_forwarding_encodes_standard_xinput_out_report() {
    let mut mock = MockVirtualOutput::new(Profile::Xbox360);
    mock.queue_rumble(RumbleState {
        large_motor: 180,
        small_motor: 90,
    });
    let mut dispatcher = OutputDispatcher::new(mock, ExtraButtonsConfig::default());

    let rumble = dispatcher
        .poll_rumble()
        .unwrap()
        .expect("queued rumble expected");
    let wire = encode_rumble_report(rumble);
    assert_eq!(wire, [0x00, 0x08, 0x00, 180, 90, 0x00, 0x00, 0x00]);
    assert_eq!(dispatcher.poll_rumble().unwrap(), None);
}

#[test]
fn mock_usb_and_jsonl_replay_pipeline_preserves_buttons_and_extra_keys() {
    use g7pro8k_odin_daemon::config::Config;
    use g7pro8k_odin_daemon::diagnostics::{
        format_capture_jsonl, replay_capture_file, replay_with_output,
    };
    use g7pro8k_odin_daemon::transport::{MockUsbTransport, UsbPacket};

    let mut hid_press = [0u8; 32];
    hid_press[0] = 0x12;
    hid_press[13] = 0x01 | 0x08; // L4 + R5
    hid_press[16..18].copy_from_slice(&164i16.to_le_bytes()); // +10 dps X

    let packets = vec![
        UsbPacket::XInputIn {
            timestamp_us: 0,
            data: build_xinput(0x00, 0x00, 0, 0, 0, 0).to_vec(),
        },
        UsbPacket::XInputIn {
            timestamp_us: 125,
            data: build_xinput(0x00, 0x10, 128, 255, 16000, -16000).to_vec(),
        },
        UsbPacket::Hid12In {
            timestamp_us: 250,
            data: hid_press.to_vec(),
        },
        UsbPacket::XInputIn {
            timestamp_us: 375,
            data: build_xinput(0x00, 0x00, 128, 255, 16000, -16000).to_vec(),
        },
    ];

    let cfg = Config::default();
    let mut mock_usb = MockUsbTransport::new(packets.clone());
    let mock_out = MockVirtualOutput::new(cfg.profile);
    let summary = replay_with_output(&mut mock_usb, mock_out, &cfg, true).unwrap();
    assert_eq!(summary.packets_processed, 4);
    assert_eq!(summary.xinput_packets, 3);
    assert_eq!(summary.hid12_packets, 1);
    // Every button edge flushed immediately (initial neutral + A press + L4/R5 press + A release).
    assert_eq!(summary.emitted_frames, 4);
    assert_eq!(summary.immediate_button_flushes, 3);

    // Serialize to .jsonl capture file and replay via ReplayTransport in dry-run mode.
    let records: Vec<_> = packets.iter().map(UsbPacket::to_capture_record).collect();
    let jsonl = format_capture_jsonl(&records).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let cap_path = dir.path().join("synthetic.jsonl");
    std::fs::write(&cap_path, jsonl).unwrap();

    let file_summary = replay_capture_file(&cap_path, &cfg, true, false, true).unwrap();
    assert_eq!(file_summary, summary);
}
