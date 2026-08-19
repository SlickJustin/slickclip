pub(crate) mod capture_test;
pub(crate) mod continuous_baseline;
pub(crate) mod encoder;
pub(crate) mod targets;

/// Stage 7.4 test knob. Keep within the vendored library's deliberately supported 1..=3 range.
pub(crate) const WGC_FRAME_POOL_BUFFER_COUNT: u32 = 2;

#[cfg(test)]
mod av1_probe;
