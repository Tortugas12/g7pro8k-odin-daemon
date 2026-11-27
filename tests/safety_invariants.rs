use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use evdev::KeyCode;
use g7pro8k_odin_daemon::config::{
    Config, ExtraButtonTarget, ExtraButtonsConfig, Profile, SUPPORTED_KEY_NAMES,
};
use g7pro8k_odin_daemon::device_registry::{
    classify_usb_device, is_allowed_device, DeviceMatch, GAMESIR_VENDOR_ID, KERNEL_QUIRK_PARAM,
    PID_XINPUT_FALLBACK_QUIRK,
};
use g7pro8k_odin_daemon::output::profiles::{
    ds4_profile_spec, extra_keys_profile_spec, key_code_from_name, xbox360_profile_spec,
    VIRTUAL_PHYS_PREFIX,
};
use g7pro8k_odin_daemon::output::{OutputDispatcher, OutputError, VirtualOutput};
use g7pro8k_odin_daemon::protocol::state::{GamepadState, RumbleState};
use g7pro8k_odin_daemon::protocol::xinput::encode_rumble_report;

#[test]
fn strict_vid_pid_allowlist_blocks_non_gamesir_and_unlisted_devices() {
    let non_gamesir_devices = [
        (0x045E, 0x028E), // Microsoft Xbox 360
        (0x054C, 0x09CC), // Sony DualShock 4
        (0x054C, 0x0CE6), // Sony DualSense
        (0x18D1, 0x4EE7), // Google ADB
        (0x05C6, 0x9091), // Qualcomm USB
        (0x2DC8, 0x3106), // 8BitDo
        (0x0000, 0x1034), // Spoofed PID with wrong VID
    ];

    let extra_pids = [0x1034, 0x028E, 0x09CC, 0x9999];
    for (vid, pid) in non_gamesir_devices {
        assert_eq!(
            classify_usb_device(vid, pid, &extra_pids),
            DeviceMatch::Ignored,
            "non-GameSir device {vid:04X}:{pid:04X} must NEVER be claimed"
        );
        assert!(!is_allowed_device(vid, pid, &extra_pids));
    }

    // Unlisted GameSir PID is ignored unless explicitly added to extra_pids.
    assert_eq!(
        classify_usb_device(GAMESIR_VENDOR_ID, 0x9999, &[]),
        DeviceMatch::Ignored
    );
    assert!(is_allowed_device(GAMESIR_VENDOR_ID, 0x9999, &[0x9999]));
}

#[test]
fn fallback_quirk_10b8_is_never_claimed_and_reports_recovery() {
    let result = classify_usb_device(GAMESIR_VENDOR_ID, PID_XINPUT_FALLBACK_QUIRK, &[0x10B8]);
    assert!(!result.is_supported());
    assert!(result.is_fallback_quirk());
    match result {
        DeviceMatch::XInputFallbackQuirk(info) => {
            assert!(info.message.contains("View + Menu"));
            assert!(info.message.contains(KERNEL_QUIRK_PARAM));
        }
        other => panic!("expected XInputFallbackQuirk, got {other:?}"),
    }
}

#[test]
fn all_supported_key_names_map_to_valid_evdev_keycodes() {
    for &name in SUPPORTED_KEY_NAMES {
        assert!(
            key_code_from_name(name).is_some(),
            "SUPPORTED_KEY_NAMES entry '{name}' missing from key_code_from_name()"
        );
    }
}

#[test]
fn virtual_device_specs_carry_unambiguous_phys_prefix_for_odin3_exclusion() {
    for spec in [
        xbox360_profile_spec(),
        ds4_profile_spec(),
        extra_keys_profile_spec(),
    ] {
        assert!(
            spec.phys.starts_with(VIRTUAL_PHYS_PREFIX),
            "virtual device '{}' must carry phys prefix '{VIRTUAL_PHYS_PREFIX}'",
            spec.name
        );
    }
}

#[test]
fn config_enforces_localhost_only_and_valid_ranges() {
    for bad_bind in [
        "0.0.0.0:8787",
        "[::]:8787",
        "192.168.1.10:8787",
        "not-an-addr",
    ] {
        let mut cfg = Config::default();
        cfg.web_ui.bind = bad_bind.to_string();
        assert!(
            cfg.validate().is_err(),
            "web_ui.bind='{bad_bind}' must be rejected"
        );
    }

    for good_bind in ["127.0.0.1:8787", "[::1]:8787", "127.0.0.1:0"] {
        let mut cfg = Config::default();
        cfg.web_ui.bind = good_bind.to_string();
        assert!(
            cfg.validate().is_ok(),
            "loopback web_ui.bind='{good_bind}' must be accepted"
        );
    }

    for bad_rate in [1, 60, 124, 1001, 8000] {
        let cfg = Config {
            emit_rate_hz: bad_rate,
            ..Default::default()
        };
        assert!(
            cfg.validate().is_err(),
            "emit_rate_hz={bad_rate} must be rejected"
        );
    }

    let mut bad_key = Config::default();
    bad_key.extra_buttons.r5 = ExtraButtonTarget::Key {
        code: "KEY_POWER".to_string(),
    };
    assert!(bad_key.validate().is_err());
}

#[test]
fn raii_drop_always_triggers_release_all() {
    struct DropSpyOutput {
        releases: Arc<AtomicUsize>,
    }

    impl VirtualOutput for DropSpyOutput {
        fn profile(&self) -> Profile {
            Profile::Xbox360
        }

        fn emit_gamepad(&mut self, _state: &GamepadState) -> Result<(), OutputError> {
            Ok(())
        }

        fn emit_extra_key(&mut self, _key: KeyCode, _pressed: bool) -> Result<(), OutputError> {
            Ok(())
        }

        fn poll_rumble(&mut self) -> Result<Option<RumbleState>, OutputError> {
            Ok(None)
        }

        fn release_all(&mut self) -> Result<(), OutputError> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    let releases = Arc::new(AtomicUsize::new(0));
    {
        let spy = DropSpyOutput {
            releases: Arc::clone(&releases),
        };
        let mut dispatcher = OutputDispatcher::new(spy, ExtraButtonsConfig::default());
        let mut held = GamepadState::default();
        held.buttons.a = true;
        held.extra.l4 = true;
        dispatcher.emit_state(&held).unwrap();
    }
    assert_eq!(releases.load(Ordering::SeqCst), 1);
}

#[test]
fn rumble_wire_report_strictly_uses_standard_xinput_header() {
    for large in [0u8, 64, 255] {
        for small in [0u8, 128, 255] {
            let wire = encode_rumble_report(RumbleState {
                large_motor: large,
                small_motor: small,
            });
            assert_eq!(wire[0], 0x00);
            assert_eq!(wire[1], 0x08);
            assert_eq!(wire[2], 0x00);
            assert_eq!(wire[3], large);
            assert_eq!(wire[4], small);
            assert_eq!(&wire[5..8], &[0x00, 0x00, 0x00]);
        }
    }
}

#[test]
fn odin3_monitor_strictly_excludes_virtual_devices_and_gamesir_and_never_grabs() {
    use g7pro8k_odin_daemon::odin_monitor::{
        is_excluded_from_odin_monitor, select_odin_builtin, OdinDeviceCandidate,
    };
    use g7pro8k_odin_daemon::output::profiles::{
        VIRTUAL_DS4_NAME, VIRTUAL_EXTRA_KEYS_NAME, VIRTUAL_EXTRA_KEYS_PHYS, VIRTUAL_GAMEPAD_PHYS,
        VIRTUAL_XBOX360_NAME,
    };

    let virtual_xbox = OdinDeviceCandidate {
        event_node: "/dev/input/event10".to_string(),
        name: VIRTUAL_XBOX360_NAME.to_string(),
        phys: VIRTUAL_GAMEPAD_PHYS.to_string(),
        vendor_id: 0x045E,
        product_id: 0x028E,
        has_gamepad_buttons: true,
        has_abs_axes: true,
    };
    let virtual_ds4 = OdinDeviceCandidate {
        event_node: "/dev/input/event11".to_string(),
        name: VIRTUAL_DS4_NAME.to_string(),
        phys: VIRTUAL_GAMEPAD_PHYS.to_string(),
        vendor_id: 0x054C,
        product_id: 0x09CC,
        has_gamepad_buttons: true,
        has_abs_axes: true,
    };
    let virtual_keys = OdinDeviceCandidate {
        event_node: "/dev/input/event12".to_string(),
        name: VIRTUAL_EXTRA_KEYS_NAME.to_string(),
        phys: VIRTUAL_EXTRA_KEYS_PHYS.to_string(),
        vendor_id: 0x3537,
        product_id: 0xFF01,
        has_gamepad_buttons: false,
        has_abs_axes: false,
    };
    let physical_gamesir = OdinDeviceCandidate {
        event_node: "/dev/input/event13".to_string(),
        name: "GameSir G7 Pro 8K".to_string(),
        phys: "usb-0000:00:14.0-1/input0".to_string(),
        vendor_id: GAMESIR_VENDOR_ID,
        product_id: 0x1035,
        has_gamepad_buttons: true,
        has_abs_axes: true,
    };
    let builtin_odin3 = OdinDeviceCandidate {
        event_node: "/dev/input/event4".to_string(),
        name: "AYN Odin3 Gamepad".to_string(),
        phys: "platform-odin-gamepad/input0".to_string(),
        vendor_id: 0x045E,
        product_id: 0x028E,
        has_gamepad_buttons: true,
        has_abs_axes: true,
    };

    assert!(is_excluded_from_odin_monitor(&virtual_xbox));
    assert!(is_excluded_from_odin_monitor(&virtual_ds4));
    assert!(is_excluded_from_odin_monitor(&virtual_keys));
    assert!(is_excluded_from_odin_monitor(&physical_gamesir));
    assert!(!is_excluded_from_odin_monitor(&builtin_odin3));

    let candidates = [
        virtual_xbox,
        virtual_ds4,
        virtual_keys,
        physical_gamesir,
        builtin_odin3.clone(),
    ];
    let chosen =
        select_odin_builtin(&candidates).expect("should select built-in Odin 3 controller");
    assert_eq!(chosen.event_node, builtin_odin3.event_node);

    // Static code invariant: odin_monitor.rs MUST NEVER invoke `.grab(` or `EVIOCGRAB`.
    let src = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/odin_monitor.rs"),
    )
    .expect("read src/odin_monitor.rs");
    for line in src.lines() {
        let code = line.split("//").next().unwrap_or("");
        assert!(
            !code.contains(".grab(") && !code.contains("EVIOCGRAB"),
            "odin_monitor.rs must NEVER call grab/EVIOCGRAB: found '{line}'"
        );
    }
}

#[test]
fn pid_file_guard_acquires_and_cleans_up_on_drop() {
    use g7pro8k_odin_daemon::diagnostics::PidFileGuard;

    let dir = tempfile::tempdir().unwrap();
    let pid_path = dir.path().join("daemon.pid");
    {
        let _guard = PidFileGuard::acquire(&pid_path).expect("acquire pid lock");
        assert!(pid_path.exists());
    }
    assert!(!pid_path.exists(), "PID file must be removed on Drop");
}
