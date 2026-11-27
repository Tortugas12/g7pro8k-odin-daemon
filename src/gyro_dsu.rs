use serde::{Deserialize, Serialize};
use tracing::{debug, trace};

/// Normalized 6-axis IMU sample extracted from GameSir HID report `0x12`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct GyroSample {
    pub timestamp_us: u64,
    /// Angular velocity in degrees per second (`[x, y, z]`).
    pub gyro_dps: [f32; 3],
    /// Linear acceleration in g (`[x, y, z]`).
    pub accel_g: [f32; 3],
}

impl GyroSample {
    /// Converts 6 raw little-endian `i16` IMU readings (`[gx, gy, gz, ax, ay, az]`)
    /// using standard MPU/ICM +/-2000 dps & +/-8g scale factors.
    #[must_use]
    pub fn from_raw_imu(timestamp_us: u64, raw: [i16; 6]) -> Self {
        const GYRO_LSB_PER_DPS: f32 = 16.4;
        const ACCEL_LSB_PER_G: f32 = 4096.0;
        Self {
            timestamp_us,
            gyro_dps: [
                f32::from(raw[0]) / GYRO_LSB_PER_DPS,
                f32::from(raw[1]) / GYRO_LSB_PER_DPS,
                f32::from(raw[2]) / GYRO_LSB_PER_DPS,
            ],
            accel_g: [
                f32::from(raw[3]) / ACCEL_LSB_PER_G,
                f32::from(raw[4]) / ACCEL_LSB_PER_G,
                f32::from(raw[5]) / ACCEL_LSB_PER_G,
            ],
        }
    }
}

/// Isolated, opt-in stub for a future Cemuhook / DSU UDP motion server (`--enable-gyro-dsu`).
///
/// Disabled by default so Switch emulator motion streaming can be enabled in a follow-up
/// release without touching the core XInput/uinput pipeline.
#[derive(Debug, Default)]
pub struct GyroDsuStub {
    enabled: bool,
    samples_received: u64,
    last_sample: Option<GyroSample>,
}

impl GyroDsuStub {
    #[must_use]
    pub fn new(enabled: bool) -> Self {
        if enabled {
            debug!(
                "gyro DSU stub enabled (--enable-gyro-dsu); UDP server emission is deferred to a future release"
            );
        }
        Self {
            enabled,
            samples_received: 0,
            last_sample: None,
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn samples_received(&self) -> u64 {
        self.samples_received
    }

    #[must_use]
    pub fn last_sample(&self) -> Option<GyroSample> {
        self.last_sample
    }

    /// Accepts a decoded IMU sample when enabled.
    ///
    /// TODO(follow-up): serialize Cemuhook DSU protocol v1001 UDP packets to `127.0.0.1:26760`.
    pub fn submit(&mut self, sample: GyroSample) {
        if !self.enabled {
            return;
        }
        self.samples_received += 1;
        self.last_sample = Some(sample);
        if self.samples_received == 1 {
            trace!(
                ?sample,
                "received first HID 0x12 IMU sample in gyro DSU stub"
            );
        }
    }
}
