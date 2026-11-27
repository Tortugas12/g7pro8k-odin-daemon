use std::collections::BTreeMap;

use evdev::{
    AbsInfo, AbsoluteAxisCode, BusType, EventType, FFEffectCode, InputEvent, InputId, KeyCode,
    UinputAbsSetup,
};

use crate::config::{ExtraButtonTarget, ExtraButtonsConfig, Profile, SUPPORTED_KEY_NAMES};
use crate::protocol::state::GamepadState;

/// Virtual device name for default Xbox 360 profile.
pub const VIRTUAL_XBOX360_NAME: &str = "GameSir G7 Pro 8K Virtual Xbox 360";

/// Virtual device name for opt-in DualShock 4 profile (matches Android/PS5 Remote Play expectations).
pub const VIRTUAL_DS4_NAME: &str = "Wireless Controller";

/// Virtual device name for the companion extra-keys keyboard.
pub const VIRTUAL_EXTRA_KEYS_NAME: &str = "GameSir G7 Pro 8K Extra Keys";

/// Sysfs `phys` prefix embedded on all virtual `uinput` devices created by this daemon so
/// `odin_monitor` can unambiguously exclude our own output devices without relying on name alone.
pub const VIRTUAL_PHYS_PREFIX: &str = "g7pro8k-odin-daemon/virtual";
pub const VIRTUAL_GAMEPAD_PHYS: &str = "g7pro8k-odin-daemon/virtual/gamepad";
pub const VIRTUAL_EXTRA_KEYS_PHYS: &str = "g7pro8k-odin-daemon/virtual/extrakeys";

/// Digital trigger threshold used for DualShock 4 `BTN_TL2` / `BTN_TR2`.
pub const DS4_TRIGGER_DIGITAL_THRESHOLD: u8 = 12;

/// Declarative specification for constructing a virtual `uinput` device.
#[derive(Debug, Clone)]
pub struct VirtualDeviceSpec {
    pub name: &'static str,
    pub phys: &'static str,
    pub input_id: InputId,
    pub keys: Vec<KeyCode>,
    pub axes: Vec<UinputAbsSetup>,
    pub ff_effects: Vec<FFEffectCode>,
    pub ff_effects_max: u32,
}

/// Returns the virtual gamepad device specification for the selected profile.
#[must_use]
pub fn gamepad_profile_spec(profile: Profile) -> VirtualDeviceSpec {
    match profile {
        Profile::Xbox360 => xbox360_profile_spec(),
        Profile::Ds4 => ds4_profile_spec(),
    }
}

/// Xbox 360 (`045E:028E`) virtual gamepad specification (default profile for A/B/X/Y icons).
#[must_use]
pub fn xbox360_profile_spec() -> VirtualDeviceSpec {
    let stick_abs = AbsInfo::new(0, -32768, 32767, 16, 128, 0);
    let trigger_abs = AbsInfo::new(0, 0, 255, 0, 0, 0);
    let hat_abs = AbsInfo::new(0, -1, 1, 0, 0, 0);

    VirtualDeviceSpec {
        name: VIRTUAL_XBOX360_NAME,
        phys: VIRTUAL_GAMEPAD_PHYS,
        input_id: InputId::new(BusType::BUS_USB, 0x045E, 0x028E, 0x0110),
        keys: vec![
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ],
        axes: vec![
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, trigger_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, trigger_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, hat_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, hat_abs),
        ],
        ff_effects: vec![FFEffectCode::FF_RUMBLE],
        ff_effects_max: 16,
    }
}

/// Sony DualShock 4 (`054C:09CC`, `"Wireless Controller"`) virtual gamepad specification
/// matching Android's `Vendor_054c_Product_09cc.kl` for PS5 Remote Play.
#[must_use]
pub fn ds4_profile_spec() -> VirtualDeviceSpec {
    let stick_abs = AbsInfo::new(128, 0, 255, 0, 0, 0);
    let trigger_abs = AbsInfo::new(0, 0, 255, 0, 0, 0);
    let hat_abs = AbsInfo::new(0, -1, 1, 0, 0, 0);

    VirtualDeviceSpec {
        name: VIRTUAL_DS4_NAME,
        phys: VIRTUAL_GAMEPAD_PHYS,
        input_id: InputId::new(BusType::BUS_USB, 0x054C, 0x09CC, 0x0100),
        keys: vec![
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_TL2,
            KeyCode::BTN_TR2,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ],
        axes: vec![
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, stick_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, trigger_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, trigger_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, hat_abs),
            UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, hat_abs),
        ],
        ff_effects: vec![FFEffectCode::FF_RUMBLE],
        ff_effects_max: 16,
    }
}

/// Companion virtual keyboard specification for independent extra buttons (`L4/R4/L5/R5`).
#[must_use]
pub fn extra_keys_profile_spec() -> VirtualDeviceSpec {
    let keys = SUPPORTED_KEY_NAMES
        .iter()
        .filter_map(|name| key_code_from_name(name))
        .collect();

    VirtualDeviceSpec {
        name: VIRTUAL_EXTRA_KEYS_NAME,
        phys: VIRTUAL_EXTRA_KEYS_PHYS,
        input_id: InputId::new(BusType::BUS_VIRTUAL, 0x3537, 0xFF01, 0x0001),
        keys,
        axes: Vec::new(),
        ff_effects: Vec::new(),
        ff_effects_max: 0,
    }
}

/// Converts a configured `KEY_*` string into a Linux [`evdev::KeyCode`].
#[must_use]
pub fn key_code_from_name(name: &str) -> Option<KeyCode> {
    let code = match name {
        "KEY_F1" => KeyCode::KEY_F1,
        "KEY_F2" => KeyCode::KEY_F2,
        "KEY_F3" => KeyCode::KEY_F3,
        "KEY_F4" => KeyCode::KEY_F4,
        "KEY_F5" => KeyCode::KEY_F5,
        "KEY_F6" => KeyCode::KEY_F6,
        "KEY_F7" => KeyCode::KEY_F7,
        "KEY_F8" => KeyCode::KEY_F8,
        "KEY_F9" => KeyCode::KEY_F9,
        "KEY_F10" => KeyCode::KEY_F10,
        "KEY_F11" => KeyCode::KEY_F11,
        "KEY_F12" => KeyCode::KEY_F12,
        "KEY_TAB" => KeyCode::KEY_TAB,
        "KEY_ESC" => KeyCode::KEY_ESC,
        "KEY_LEFTSHIFT" => KeyCode::KEY_LEFTSHIFT,
        "KEY_RIGHTSHIFT" => KeyCode::KEY_RIGHTSHIFT,
        "KEY_LEFTCTRL" => KeyCode::KEY_LEFTCTRL,
        "KEY_RIGHTCTRL" => KeyCode::KEY_RIGHTCTRL,
        "KEY_LEFTALT" => KeyCode::KEY_LEFTALT,
        "KEY_RIGHTALT" => KeyCode::KEY_RIGHTALT,
        "KEY_SPACE" => KeyCode::KEY_SPACE,
        "KEY_ENTER" => KeyCode::KEY_ENTER,
        "KEY_BACKSPACE" => KeyCode::KEY_BACKSPACE,
        "KEY_1" => KeyCode::KEY_1,
        "KEY_2" => KeyCode::KEY_2,
        "KEY_3" => KeyCode::KEY_3,
        "KEY_4" => KeyCode::KEY_4,
        "KEY_5" => KeyCode::KEY_5,
        "KEY_6" => KeyCode::KEY_6,
        "KEY_7" => KeyCode::KEY_7,
        "KEY_8" => KeyCode::KEY_8,
        "KEY_9" => KeyCode::KEY_9,
        "KEY_0" => KeyCode::KEY_0,
        "KEY_Q" => KeyCode::KEY_Q,
        "KEY_E" => KeyCode::KEY_E,
        "KEY_R" => KeyCode::KEY_R,
        "KEY_F" => KeyCode::KEY_F,
        "KEY_G" => KeyCode::KEY_G,
        "KEY_I" => KeyCode::KEY_I,
        "KEY_M" => KeyCode::KEY_M,
        "KEY_C" => KeyCode::KEY_C,
        "KEY_V" => KeyCode::KEY_V,
        "KEY_X" => KeyCode::KEY_X,
        "KEY_Z" => KeyCode::KEY_Z,
        _ => return None,
    };
    Some(code)
}

/// Converts XInput up-positive Y (`-32768..=32767`) into Linux `evdev` down-positive Y.
#[must_use]
pub fn invert_xinput_y_i16(y: i16) -> i32 {
    (-i32::from(y)).clamp(-32768, 32767)
}

/// Scales signed XInput X (`-32768..=32767`, right positive) into DS4 `0..=255` centered at `128`.
#[must_use]
pub fn scale_xinput_x_to_ds4(x: i16) -> i32 {
    ((i32::from(x) + 32768) >> 8).clamp(0, 255)
}

/// Scales signed XInput Y (`-32768..=32767`, up positive) into DS4 `0..=255` (`0` = up, `128` = center, `255` = down).
#[must_use]
pub fn scale_xinput_y_to_ds4(y: i16) -> i32 {
    ((32768 - i32::from(y)) >> 8).clamp(0, 255)
}

/// Applies configured extra-button mappings:
/// - Any `ExtraButtonTarget::Gamepad { button }` is OR-ed into the returned [`GamepadState`].
/// - Any `ExtraButtonTarget::Key { code }` produces a deduplicated map of `KeyCode -> pressed`.
#[must_use]
pub fn resolve_extra_buttons(
    state: &GamepadState,
    cfg: &ExtraButtonsConfig,
) -> (GamepadState, BTreeMap<KeyCode, bool>) {
    let mut effective = *state;
    let mut key_states: BTreeMap<KeyCode, bool> = BTreeMap::new();

    let bindings = [
        (state.extra.l4, &cfg.l4),
        (state.extra.r4, &cfg.r4),
        (state.extra.l5, &cfg.l5),
        (state.extra.r5, &cfg.r5),
    ];

    for (pressed, target) in bindings {
        match target {
            ExtraButtonTarget::Gamepad { button } => {
                if pressed {
                    effective.buttons.set(*button, true);
                }
            }
            ExtraButtonTarget::Key { code } => {
                if let Some(key_code) = key_code_from_name(code) {
                    let entry = key_states.entry(key_code).or_insert(false);
                    *entry |= pressed;
                }
            }
            ExtraButtonTarget::Disabled => {}
        }
    }

    (effective, key_states)
}

/// Translates a canonical [`GamepadState`] into the exact Linux `evdev` events for `profile`.
#[must_use]
pub fn map_gamepad_events(profile: Profile, state: &GamepadState) -> Vec<InputEvent> {
    let b = state.buttons;
    let mut events = Vec::with_capacity(24);

    let (lx, ly, rx, ry) = match profile {
        Profile::Xbox360 => (
            i32::from(state.lx),
            invert_xinput_y_i16(state.ly),
            i32::from(state.rx),
            invert_xinput_y_i16(state.ry),
        ),
        Profile::Ds4 => (
            scale_xinput_x_to_ds4(state.lx),
            scale_xinput_y_to_ds4(state.ly),
            scale_xinput_x_to_ds4(state.rx),
            scale_xinput_y_to_ds4(state.ry),
        ),
    };

    events.push(abs_event(AbsoluteAxisCode::ABS_X, lx));
    events.push(abs_event(AbsoluteAxisCode::ABS_Y, ly));
    events.push(abs_event(AbsoluteAxisCode::ABS_RX, rx));
    events.push(abs_event(AbsoluteAxisCode::ABS_RY, ry));
    events.push(abs_event(AbsoluteAxisCode::ABS_Z, i32::from(state.lt)));
    events.push(abs_event(AbsoluteAxisCode::ABS_RZ, i32::from(state.rt)));
    events.push(abs_event(AbsoluteAxisCode::ABS_HAT0X, b.dpad_hat_x()));
    events.push(abs_event(AbsoluteAxisCode::ABS_HAT0Y, b.dpad_hat_y()));

    events.push(key_event(KeyCode::BTN_SOUTH, b.a));
    events.push(key_event(KeyCode::BTN_EAST, b.b));
    events.push(key_event(KeyCode::BTN_WEST, b.x));
    events.push(key_event(KeyCode::BTN_NORTH, b.y));
    events.push(key_event(KeyCode::BTN_TL, b.lb));
    events.push(key_event(KeyCode::BTN_TR, b.rb));
    events.push(key_event(KeyCode::BTN_SELECT, b.view));
    events.push(key_event(KeyCode::BTN_START, b.menu));
    events.push(key_event(KeyCode::BTN_MODE, b.guide));
    events.push(key_event(KeyCode::BTN_THUMBL, b.ls_click));
    events.push(key_event(KeyCode::BTN_THUMBR, b.rs_click));

    if profile == Profile::Ds4 {
        events.push(key_event(
            KeyCode::BTN_TL2,
            state.lt >= DS4_TRIGGER_DIGITAL_THRESHOLD,
        ));
        events.push(key_event(
            KeyCode::BTN_TR2,
            state.rt >= DS4_TRIGGER_DIGITAL_THRESHOLD,
        ));
    }

    events
}

#[must_use]
pub fn key_event(code: KeyCode, pressed: bool) -> InputEvent {
    InputEvent::new(EventType::KEY.0, code.0, i32::from(pressed))
}

#[must_use]
pub fn abs_event(code: AbsoluteAxisCode, value: i32) -> InputEvent {
    InputEvent::new(EventType::ABSOLUTE.0, code.0, value)
}
