pub mod profiles;
pub mod uinput_backend;

use std::collections::{BTreeMap, VecDeque};

use evdev::{InputEvent, KeyCode};
use thiserror::Error;

use crate::config::{ExtraButtonsConfig, Profile};
use crate::output::profiles::{map_gamepad_events, resolve_extra_buttons};
use crate::protocol::state::{GamepadState, RumbleState};

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("virtual input I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to create virtual uinput device '{device}': {source}")]
    DeviceCreation {
        device: &'static str,
        #[source]
        source: std::io::Error,
    },
}

/// Abstract virtual output layer allowing headless tests with [`MockVirtualOutput`]
/// and real Linux/Android `/dev/uinput` output with [`uinput_backend::UinputVirtualOutput`].
pub trait VirtualOutput: Send {
    fn profile(&self) -> Profile;
    fn emit_gamepad(&mut self, state: &GamepadState) -> Result<(), OutputError>;
    fn emit_extra_key(&mut self, key: KeyCode, pressed: bool) -> Result<(), OutputError>;
    fn poll_rumble(&mut self) -> Result<Option<RumbleState>, OutputError>;
    fn release_all(&mut self) -> Result<(), OutputError>;
}

/// Stateful dispatcher that resolves extra-button mappings (gamepad mirrors + companion keyboard
/// transitions) and forwards them cleanly to any [`VirtualOutput`] implementation.
///
/// Dropping [`OutputDispatcher`] deterministically invokes `release_all()` on the underlying output.
pub struct OutputDispatcher<O: VirtualOutput> {
    output: O,
    extra_buttons: ExtraButtonsConfig,
    pressed_keys: BTreeMap<KeyCode, bool>,
}

impl<O: VirtualOutput> OutputDispatcher<O> {
    pub fn new(output: O, extra_buttons: ExtraButtonsConfig) -> Self {
        Self {
            output,
            extra_buttons,
            pressed_keys: BTreeMap::new(),
        }
    }

    pub fn update_extra_buttons(
        &mut self,
        extra_buttons: ExtraButtonsConfig,
    ) -> Result<(), OutputError> {
        // Release any previously held extra keys before switching bindings so no keys get stuck.
        let held: Vec<KeyCode> = self
            .pressed_keys
            .iter()
            .filter_map(|(&k, &pressed)| pressed.then_some(k))
            .collect();
        for key in held {
            self.output.emit_extra_key(key, false)?;
            self.pressed_keys.insert(key, false);
        }
        self.extra_buttons = extra_buttons;
        Ok(())
    }

    pub fn emit_state(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        let (effective_gamepad, desired_keys) = resolve_extra_buttons(state, &self.extra_buttons);
        self.output.emit_gamepad(&effective_gamepad)?;

        // Release any previously pressed key no longer present in `desired_keys`.
        let stale_keys: Vec<KeyCode> = self
            .pressed_keys
            .iter()
            .filter_map(|(&key, &was_pressed)| {
                (was_pressed && !desired_keys.contains_key(&key)).then_some(key)
            })
            .collect();
        for key in stale_keys {
            self.output.emit_extra_key(key, false)?;
            self.pressed_keys.insert(key, false);
        }

        // Emit edges for active extra-key bindings.
        for (key, pressed) in desired_keys {
            let prev = self.pressed_keys.get(&key).copied().unwrap_or(false);
            if prev != pressed {
                self.output.emit_extra_key(key, pressed)?;
                self.pressed_keys.insert(key, pressed);
            }
        }

        Ok(())
    }

    pub fn poll_rumble(&mut self) -> Result<Option<RumbleState>, OutputError> {
        self.output.poll_rumble()
    }

    pub fn release_all(&mut self) -> Result<(), OutputError> {
        self.pressed_keys.clear();
        self.output.release_all()
    }

    pub fn inner(&self) -> &O {
        &self.output
    }

    pub fn inner_mut(&mut self) -> &mut O {
        &mut self.output
    }
}

impl<O: VirtualOutput> Drop for OutputDispatcher<O> {
    fn drop(&mut self) {
        let _ = self.release_all();
    }
}

/// Recorded virtual output action for deterministic headless testing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockOutputRecord {
    Gamepad {
        state: GamepadState,
        events: Vec<InputEvent>,
    },
    ExtraKey {
        key: KeyCode,
        pressed: bool,
    },
    ReleaseAll,
}

/// Headless in-memory [`VirtualOutput`] implementation recording every emitted event sequence.
#[derive(Debug, Default)]
pub struct MockVirtualOutput {
    profile: Profile,
    records: Vec<MockOutputRecord>,
    active_keys: BTreeMap<KeyCode, bool>,
    last_gamepad_state: Option<GamepadState>,
    rumble_queue: VecDeque<RumbleState>,
    release_all_calls: usize,
}

impl MockVirtualOutput {
    #[must_use]
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            ..Default::default()
        }
    }

    pub fn queue_rumble(&mut self, rumble: RumbleState) {
        self.rumble_queue.push_back(rumble);
    }

    #[must_use]
    pub fn records(&self) -> &[MockOutputRecord] {
        &self.records
    }

    #[must_use]
    pub fn last_gamepad_state(&self) -> Option<GamepadState> {
        self.last_gamepad_state
    }

    #[must_use]
    pub fn active_keys(&self) -> &BTreeMap<KeyCode, bool> {
        &self.active_keys
    }

    #[must_use]
    pub fn release_all_calls(&self) -> usize {
        self.release_all_calls
    }
}

impl VirtualOutput for MockVirtualOutput {
    fn profile(&self) -> Profile {
        self.profile
    }

    fn emit_gamepad(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        let events = map_gamepad_events(self.profile, state);
        self.last_gamepad_state = Some(*state);
        self.records.push(MockOutputRecord::Gamepad {
            state: *state,
            events,
        });
        Ok(())
    }

    fn emit_extra_key(&mut self, key: KeyCode, pressed: bool) -> Result<(), OutputError> {
        self.active_keys.insert(key, pressed);
        self.records
            .push(MockOutputRecord::ExtraKey { key, pressed });
        Ok(())
    }

    fn poll_rumble(&mut self) -> Result<Option<RumbleState>, OutputError> {
        Ok(self.rumble_queue.pop_front())
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        self.release_all_calls += 1;
        let neutral = GamepadState::default();
        self.emit_gamepad(&neutral)?;
        let held: Vec<KeyCode> = self
            .active_keys
            .iter()
            .filter_map(|(&k, &p)| p.then_some(k))
            .collect();
        for key in held {
            self.emit_extra_key(key, false)?;
        }
        self.records.push(MockOutputRecord::ReleaseAll);
        Ok(())
    }
}
