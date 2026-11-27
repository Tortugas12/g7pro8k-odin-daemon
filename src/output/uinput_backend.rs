use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::io;
use std::os::fd::AsRawFd;

use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, EventSummary, FFEffectCode, FFEffectKind, KeyCode, UInputCode};
use tracing::{debug, trace, warn};

use crate::config::Profile;
use crate::output::profiles::{
    extra_keys_profile_spec, gamepad_profile_spec, key_event, map_gamepad_events, VirtualDeviceSpec,
};
use crate::output::{OutputError, VirtualOutput};
use crate::protocol::state::{GamepadState, RumbleState};

const MAX_FF_SLOTS: i16 = 16;

/// Linux `/dev/uinput` backend hosting both the active virtual gamepad (Xbox 360 or DS4)
/// and the companion extra-keys virtual keyboard.
///
/// Safety invariant (Deterministic RAII Cleanup): dropping [`UinputVirtualOutput`] calls
/// [`VirtualOutput::release_all`] before closing the `uinput` descriptors so zero stuck buttons
/// or off-center axes remain on Android.
pub struct UinputVirtualOutput {
    profile: Profile,
    gamepad: VirtualDevice,
    extra_keys: VirtualDevice,
    ff_effects: HashMap<i16, RumbleState>,
    playing_effect_id: Option<i16>,
    pressed_extra_keys: HashSet<KeyCode>,
    cleaned_up: bool,
}

impl UinputVirtualOutput {
    /// Creates both virtual `/dev/uinput` devices for `profile`.
    pub fn new(profile: Profile) -> Result<Self, OutputError> {
        let gp_spec = gamepad_profile_spec(profile);
        let gamepad = build_virtual_device(&gp_spec)?;
        set_fd_nonblocking(gamepad.as_raw_fd()).map_err(|source| OutputError::DeviceCreation {
            device: gp_spec.name,
            source,
        })?;

        let kb_spec = extra_keys_profile_spec();
        let extra_keys = build_virtual_device(&kb_spec)?;

        debug!(
            profile = ?profile,
            gamepad_name = gp_spec.name,
            extra_keys_name = kb_spec.name,
            "created uinput virtual devices"
        );

        Ok(Self {
            profile,
            gamepad,
            extra_keys,
            ff_effects: HashMap::new(),
            playing_effect_id: None,
            pressed_extra_keys: HashSet::new(),
            cleaned_up: false,
        })
    }

    fn allocate_ff_slot(&self) -> i16 {
        for slot in 0..MAX_FF_SLOTS {
            if !self.ff_effects.contains_key(&slot) {
                return slot;
            }
        }
        0
    }
}

impl VirtualOutput for UinputVirtualOutput {
    fn profile(&self) -> Profile {
        self.profile
    }

    fn emit_gamepad(&mut self, state: &GamepadState) -> Result<(), OutputError> {
        self.cleaned_up = false;
        let events = map_gamepad_events(self.profile, state);
        self.gamepad.emit(&events)?;
        Ok(())
    }

    fn emit_extra_key(&mut self, key: KeyCode, pressed: bool) -> Result<(), OutputError> {
        self.cleaned_up = false;
        if pressed {
            self.pressed_extra_keys.insert(key);
        } else {
            self.pressed_extra_keys.remove(&key);
        }
        let ev = key_event(key, pressed);
        self.extra_keys.emit(&[ev])?;
        Ok(())
    }

    fn poll_rumble(&mut self) -> Result<Option<RumbleState>, OutputError> {
        let events: Vec<_> = match self.gamepad.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(err) => return Err(OutputError::Io(err)),
        };

        let mut latest_rumble: Option<RumbleState> = None;

        for event in events {
            match event.destructure() {
                EventSummary::UInput(uinput_ev, UInputCode::UI_FF_UPLOAD, _) => {
                    match self.gamepad.process_ff_upload(uinput_ev) {
                        Ok(mut upload) => {
                            let id = if upload.effect_id() < 0 {
                                let slot = self.allocate_ff_slot();
                                upload.set_effect_id(slot);
                                slot
                            } else {
                                upload.effect_id()
                            };

                            let rumble = match upload.effect().kind {
                                FFEffectKind::Rumble {
                                    strong_magnitude,
                                    weak_magnitude,
                                } => RumbleState {
                                    large_motor: (strong_magnitude >> 8) as u8,
                                    small_motor: (weak_magnitude >> 8) as u8,
                                },
                                _ => RumbleState::default(),
                            };

                            self.ff_effects.insert(id, rumble);
                            upload.set_retval(0);
                            trace!(effect_id = id, ?rumble, "uploaded virtual FF rumble effect");
                        }
                        Err(err) => warn!(error = %err, "failed to process UI_FF_UPLOAD"),
                    }
                }
                EventSummary::UInput(uinput_ev, UInputCode::UI_FF_ERASE, _) => {
                    match self.gamepad.process_ff_erase(uinput_ev) {
                        Ok(mut erase) => {
                            let id = erase.effect_id() as i16;
                            self.ff_effects.remove(&id);
                            if self.playing_effect_id == Some(id) {
                                self.playing_effect_id = None;
                                latest_rumble = Some(RumbleState::default());
                            }
                            erase.set_retval(0);
                            trace!(effect_id = id, "erased virtual FF rumble effect");
                        }
                        Err(err) => warn!(error = %err, "failed to process UI_FF_ERASE"),
                    }
                }
                EventSummary::ForceFeedback(_, code, value) => {
                    let effect_id = code.0 as i16;
                    if value > 0 {
                        if let Some(&rumble) = self.ff_effects.get(&effect_id) {
                            self.playing_effect_id = Some(effect_id);
                            latest_rumble = Some(rumble);
                        }
                    } else if self.playing_effect_id == Some(effect_id) || value == 0 {
                        self.playing_effect_id = None;
                        latest_rumble = Some(RumbleState::default());
                    }
                }
                _ => {}
            }
        }

        Ok(latest_rumble)
    }

    fn release_all(&mut self) -> Result<(), OutputError> {
        if self.cleaned_up {
            return Ok(());
        }

        let neutral = GamepadState::default();
        let gp_events = map_gamepad_events(self.profile, &neutral);
        self.gamepad.emit(&gp_events)?;

        if !self.pressed_extra_keys.is_empty() {
            let release_events: Vec<_> = self
                .pressed_extra_keys
                .drain()
                .map(|key| key_event(key, false))
                .collect();
            self.extra_keys.emit(&release_events)?;
        }

        self.cleaned_up = true;
        Ok(())
    }
}

impl Drop for UinputVirtualOutput {
    fn drop(&mut self) {
        if let Err(err) = self.release_all() {
            warn!(error = %err, "error releasing virtual uinput state during Drop");
        }
    }
}

fn build_virtual_device(spec: &VirtualDeviceSpec) -> Result<VirtualDevice, OutputError> {
    let mut builder = VirtualDevice::builder()
        .map_err(|source| OutputError::DeviceCreation {
            device: spec.name,
            source,
        })?
        .name(spec.name)
        .input_id(spec.input_id.clone());

    let phys = CString::new(spec.phys).map_err(|_| OutputError::DeviceCreation {
        device: spec.name,
        source: io::Error::new(io::ErrorKind::InvalidInput, "phys contains NUL byte"),
    })?;
    builder = builder
        .with_phys(&phys)
        .map_err(|source| OutputError::DeviceCreation {
            device: spec.name,
            source,
        })?;

    if !spec.keys.is_empty() {
        let mut keys = AttributeSet::<KeyCode>::new();
        for &key in &spec.keys {
            keys.insert(key);
        }
        builder = builder
            .with_keys(&keys)
            .map_err(|source| OutputError::DeviceCreation {
                device: spec.name,
                source,
            })?;
    }

    for axis in &spec.axes {
        builder =
            builder
                .with_absolute_axis(axis)
                .map_err(|source| OutputError::DeviceCreation {
                    device: spec.name,
                    source,
                })?;
    }

    if !spec.ff_effects.is_empty() {
        let mut ff = AttributeSet::<FFEffectCode>::new();
        for &effect in &spec.ff_effects {
            ff.insert(effect);
        }
        builder = builder
            .with_ff(&ff)
            .map_err(|source| OutputError::DeviceCreation {
                device: spec.name,
                source,
            })?
            .with_ff_effects_max(spec.ff_effects_max);
    }

    builder
        .build()
        .map_err(|source| OutputError::DeviceCreation {
            device: spec.name,
            source,
        })
}

fn set_fd_nonblocking(fd: i32) -> io::Result<()> {
    // SAFETY: `fcntl(F_GETFL/F_SETFL)` on a valid open file descriptor owned by `VirtualDevice`
    // is a standard POSIX non-blocking mode toggle with no memory-safety preconditions.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
