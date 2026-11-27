use g7pro8k_odin_daemon::coalescer::Coalescer;
use g7pro8k_odin_daemon::protocol::state::GamepadState;

#[test]
fn pure_analog_8khz_sweep_is_capped_and_keeps_latest_sample() {
    let mut coalescer = Coalescer::new(1000); // 1000 us interval
    let mut emitted = Vec::new();

    // Feed 80 packets at 8kHz (every 125 us = 10 ms total).
    for i in 0u64..80 {
        let state = GamepadState {
            lx: (i as i16) * 200,
            ..Default::default()
        };
        if let Some(frame) = coalescer.push(state, i * 125) {
            emitted.push((i * 125, frame));
        }
    }

    // Flush the trailing pending sample once the final 1000 us window elapses.
    if let Some(frame) = coalescer.flush_due(80 * 125) {
        emitted.push((80 * 125, frame));
    }

    // Expect roughly 11 frames (t=0, 1000, 2000, ..., 10000 us) from 80 packets.
    assert_eq!(emitted.len(), 11);
    assert_eq!(emitted.first().unwrap().1.lx, 0);
    // Final resting stick value (i = 79 -> 15800) MUST NOT be lost.
    assert_eq!(emitted.last().unwrap().1.lx, 79 * 200);
    assert!(!coalescer.has_pending());
}

#[test]
fn rapid_button_tap_inside_coalesce_window_is_never_dropped() {
    // Aggressive 250 Hz cap (4000 us window).
    let mut coalescer = Coalescer::new(250);

    // Baseline neutral sample at t=0.
    let neutral = GamepadState::default();
    assert_eq!(coalescer.push(neutral, 0), Some(neutral));

    // 125 us later: analog drift -> coalesced.
    let drift = GamepadState {
        lx: 500,
        ..Default::default()
    };
    assert_eq!(coalescer.push(drift, 125), None);

    // 250 us later: fast A press -> MUST emit immediately!
    let mut pressed = drift;
    pressed.buttons.a = true;
    assert_eq!(coalescer.push(pressed, 250), Some(pressed));

    // 375 us later: fast A release -> MUST emit immediately!
    let mut released = drift;
    released.buttons.a = false;
    assert_eq!(coalescer.push(released, 375), Some(released));

    assert_eq!(coalescer.stats().immediate_button_flushes, 2);
}

#[test]
fn extra_button_edges_trigger_immediate_flushes() {
    let mut coalescer = Coalescer::new(250);
    let base = GamepadState::default();
    assert_eq!(coalescer.push(base, 0), Some(base));

    let mut l4_down = base;
    l4_down.extra.l4 = true;
    assert_eq!(coalescer.push(l4_down, 125), Some(l4_down));

    let mut r5_down = l4_down;
    r5_down.extra.r5 = true;
    assert_eq!(coalescer.push(r5_down, 250), Some(r5_down));

    let mut both_up = base;
    both_up.extra.l4 = false;
    both_up.extra.r5 = false;
    assert_eq!(coalescer.push(both_up, 375), Some(both_up));

    assert_eq!(coalescer.stats().immediate_button_flushes, 3);
}

#[test]
fn uncapped_mode_passes_every_frame_immediately() {
    let mut coalescer = Coalescer::new(0);
    assert_eq!(coalescer.min_interval_us(), 0);

    for i in 0u64..16 {
        let state = GamepadState {
            rx: (i as i16) * 100,
            ..Default::default()
        };
        assert_eq!(coalescer.push(state, i * 125), Some(state));
    }

    assert_eq!(coalescer.stats().emitted, 16);
    assert_eq!(coalescer.stats().coalesced_analog, 0);
}

#[test]
fn rate_change_and_reset_behave_cleanly() {
    let mut coalescer = Coalescer::new(1000);
    assert_eq!(coalescer.min_interval_us(), 1000);

    coalescer.set_emit_rate_hz(500);
    assert_eq!(coalescer.min_interval_us(), 2000);

    let s1 = GamepadState {
        lt: 10,
        ..Default::default()
    };
    assert_eq!(coalescer.push(s1, 0), Some(s1));

    let s2 = GamepadState {
        lt: 20,
        ..Default::default()
    };
    assert_eq!(coalescer.push(s2, 1000), None);
    assert!(coalescer.has_pending());
    assert_eq!(coalescer.flush_due(1999), None);
    assert_eq!(coalescer.flush_due(2000), Some(s2));

    coalescer.reset();
    assert!(!coalescer.has_pending());
    assert_eq!(coalescer.push(s1, 2100), Some(s1));
}
