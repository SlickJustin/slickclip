use std::time::Instant;

use serde::Serialize;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

const HUNDRED_NS_PER_SECOND: i128 = 10_000_000;

/// One calibration shared by WGC and every WASAPI track. WGC SystemRelativeTime
/// and WASAPI's GetBuffer QPC position are both QPC-based 100 ns values. The
/// raw counter/frequency are retained as the authoritative calibration.
#[derive(Clone, Debug)]
pub struct ReplaySessionClock {
    pub session_start_qpc: i64,
    pub qpc_frequency: i64,
    pub session_start_qpc_100ns: i64,
    pub started: Instant,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayClockStatus {
    pub session_start_qpc: Option<i64>,
    pub qpc_frequency: Option<i64>,
    pub session_start_qpc_100ns: Option<i64>,
    pub timing_domain: String,
}

impl ReplaySessionClock {
    pub fn create() -> Result<Self, String> {
        let mut frequency = 0i64;
        let mut start = 0i64;
        unsafe {
            QueryPerformanceFrequency(&mut frequency)
                .map_err(|error| format!("Could not read QPC frequency: {error}"))?;
            QueryPerformanceCounter(&mut start)
                .map_err(|error| format!("Could not read replay session QPC: {error}"))?;
        }
        Self::from_calibration(start, frequency, Instant::now())
    }

    fn from_calibration(start: i64, frequency: i64, started: Instant) -> Result<Self, String> {
        if frequency <= 0 {
            return Err("Windows reported an invalid QPC frequency.".to_string());
        }
        Ok(Self {
            session_start_qpc: start,
            qpc_frequency: frequency,
            session_start_qpc_100ns: raw_qpc_to_100ns(start, frequency),
            started,
        })
    }

    pub fn normalized_qpc_to_session_100ns(&self, qpc_100ns: i64) -> i64 {
        qpc_100ns.saturating_sub(self.session_start_qpc_100ns)
    }

    pub fn raw_qpc_to_session_100ns(&self, qpc: i64) -> i64 {
        raw_qpc_to_100ns(qpc, self.qpc_frequency).saturating_sub(self.session_start_qpc_100ns)
    }

    pub fn now_qpc_100ns(&self) -> Result<i64, String> {
        let mut now = 0i64;
        unsafe { QueryPerformanceCounter(&mut now) }
            .map_err(|error| format!("Could not query the replay QPC clock: {error}"))?;
        Ok(raw_qpc_to_100ns(now, self.qpc_frequency))
    }

    pub fn status(&self) -> ReplayClockStatus {
        ReplayClockStatus {
            session_start_qpc: Some(self.session_start_qpc),
            qpc_frequency: Some(self.qpc_frequency),
            session_start_qpc_100ns: Some(self.session_start_qpc_100ns),
            timing_domain: "Windows QPC; WGC SystemRelativeTime and WASAPI qpcPosition are normalized QPC in 100-ns units".to_string(),
        }
    }
}

fn raw_qpc_to_100ns(qpc: i64, frequency: i64) -> i64 {
    ((i128::from(qpc) * HUNDRED_NS_PER_SECOND) / i128::from(frequency))
        .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_raw_qpc_to_session_relative_100ns_without_float_error() {
        let clock = ReplaySessionClock::from_calibration(30_000, 10_000, Instant::now()).unwrap();
        assert_eq!(clock.session_start_qpc_100ns, 30_000_000);
        assert_eq!(clock.raw_qpc_to_session_100ns(31_250), 1_250_000);
        assert_eq!(clock.normalized_qpc_to_session_100ns(31_250_000), 1_250_000);
    }

    #[test]
    fn rejects_invalid_frequency() {
        assert!(ReplaySessionClock::from_calibration(1, 0, Instant::now()).is_err());
    }
}
