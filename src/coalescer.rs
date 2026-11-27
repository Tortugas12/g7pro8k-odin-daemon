use crate::protocol::state::GamepadState;

/// Telemetry statistics tracked by [`Coalescer`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CoalescerStats {
    pub received: u64,
    pub emitted: u64,
    pub coalesced_analog: u64,
    pub immediate_button_flushes: u64,
}

/// Deterministic, edge-preserving 8kHz-to-target-Hz input coalescer.
///
/// Safety invariant:
/// - Pure analog stick/trigger updates are coalesced to `emit_rate_hz` (always keeping the latest sample).
/// - ANY button press or release edge (standard or `L4/R4/L5/R5` extra buttons) triggers an
///   **immediate** flush so fast taps are never dropped.
#[derive(Debug, Clone)]
pub struct Coalescer {
    emit_rate_hz: u32,
    min_interval_us: u64,
    last_emitted: Option<GamepadState>,
    last_emit_us: Option<u64>,
    pending: Option<GamepadState>,
    stats: CoalescerStats,
}

impl Coalescer {
    #[must_use]
    pub fn new(emit_rate_hz: u32) -> Self {
        Self {
            emit_rate_hz,
            min_interval_us: compute_interval_us(emit_rate_hz),
            last_emitted: None,
            last_emit_us: None,
            pending: None,
            stats: CoalescerStats::default(),
        }
    }

    #[must_use]
    pub fn emit_rate_hz(&self) -> u32 {
        self.emit_rate_hz
    }

    #[must_use]
    pub fn min_interval_us(&self) -> u64 {
        self.min_interval_us
    }

    #[must_use]
    pub fn stats(&self) -> CoalescerStats {
        self.stats
    }

    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Updates the target rate at runtime (e.g. after live config hot-reload).
    pub fn set_emit_rate_hz(&mut self, emit_rate_hz: u32) {
        self.emit_rate_hz = emit_rate_hz;
        self.min_interval_us = compute_interval_us(emit_rate_hz);
    }

    /// Clears coalescer state on USB disconnect or pause.
    pub fn reset(&mut self) {
        self.last_emitted = None;
        self.last_emit_us = None;
        self.pending = None;
    }

    /// Pushes a new canonical controller state sampled at `now_us` microseconds.
    ///
    /// Returns `Some(state)` when a frame should be emitted immediately to `uinput`,
    /// or `None` when the analog update was coalesced for the next interval tick.
    pub fn push(&mut self, state: GamepadState, now_us: u64) -> Option<GamepadState> {
        self.stats.received += 1;

        // First sample always emits immediately so initial state is synchronized.
        let Some(prev) = self.last_emitted else {
            return Some(self.record_emit(state, now_us));
        };

        // Uncapped passthrough (`emit_rate_hz == 0`) emits every sample immediately.
        if self.emit_rate_hz == 0 {
            if state.buttons_changed(&prev) {
                self.stats.immediate_button_flushes += 1;
            }
            return Some(self.record_emit(state, now_us));
        }

        // Lossless button-edge invariant: any standard or extra button edge bypasses the timer.
        if state.buttons_changed(&prev) {
            self.stats.immediate_button_flushes += 1;
            return Some(self.record_emit(state, now_us));
        }

        // Identical sample with no pending change needs no emission.
        if state == prev && self.pending.is_none() {
            self.stats.coalesced_analog += 1;
            return None;
        }

        let elapsed = self
            .last_emit_us
            .map_or(self.min_interval_us, |last| now_us.saturating_sub(last));

        if elapsed >= self.min_interval_us {
            Some(self.record_emit(state, now_us))
        } else {
            self.pending = Some(state);
            self.stats.coalesced_analog += 1;
            None
        }
    }

    /// Flushes a deferred analog sample once the coalescing interval has elapsed.
    pub fn flush_due(&mut self, now_us: u64) -> Option<GamepadState> {
        let pending = self.pending?;
        let elapsed = self
            .last_emit_us
            .map_or(self.min_interval_us, |last| now_us.saturating_sub(last));

        if elapsed >= self.min_interval_us {
            Some(self.record_emit(pending, now_us))
        } else {
            None
        }
    }

    fn record_emit(&mut self, state: GamepadState, now_us: u64) -> GamepadState {
        self.last_emitted = Some(state);
        self.last_emit_us = Some(now_us);
        self.pending = None;
        self.stats.emitted += 1;
        state
    }
}

fn compute_interval_us(emit_rate_hz: u32) -> u64 {
    if emit_rate_hz == 0 {
        0
    } else {
        1_000_000 / u64::from(emit_rate_hz)
    }
}
