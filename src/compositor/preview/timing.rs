/// Fixed rolling reservoir: measurement storage does not grow with session length.
#[derive(Clone)]
pub(super) struct Timings {
    values: [u32; 1024],
    cursor: usize,
    count: usize,
}
impl Default for Timings {
    fn default() -> Self {
        Self {
            values: [0; 1024],
            cursor: 0,
            count: 0,
        }
    }
}
impl Timings {
    pub fn push(&mut self, micros: u64) {
        self.values[self.cursor] = micros.min(u32::MAX as u64) as u32;
        self.cursor = (self.cursor + 1) % self.values.len();
        self.count = (self.count + 1).min(self.values.len());
    }

    pub fn summary(&self) -> super::FrameTimes {
        let mut values = self.values;
        values[..self.count].sort_unstable();
        let at = |n: usize| {
            if self.count == 0 {
                0
            } else {
                values[((self.count - 1) * n) / 100] as u64
            }
        };
        super::FrameTimes {
            samples: self.count,
            p50_us: at(50),
            p95_us: at(95),
            p99_us: at(99),
        }
    }
}
#[derive(Default)]
pub(super) struct Metrics {
    pub session: u64,
    pub presented_frames: u64,
    pub first_present_us: u64,
    pub cpu: Timings,
    pub gpu: Timings,
    pub last_gpu_us: u64,
    pub last_presented_time: f64,
    pub presentation_interval: Timings,
    pub submission_to_present: Timings,
    pub callback_to_owner: Timings,
    pub presentation_lateness: Timings,
    pub capture_to_present: Timings,
}
