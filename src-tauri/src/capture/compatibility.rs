#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Legacy wire values retained so an existing v1.0.3 preferences file or stale frontend can
/// still deserialize safely. Replay no longer uses this value to choose or switch video APIs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CaptureMode {
    #[default]
    Auto,
    GameCapture,
    ScreenCapture,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoBackend {
    Dxgi,
    FfmpegDdagrab,
}

impl VideoBackend {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Dxgi => "DXGI",
            Self::FfmpegDdagrab => "FFmpeg ddagrab",
        }
    }
}

/// Initial window geometry is diagnostic metadata only. It never changes the selected display
/// or capture backend after a Replay session begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresentationClass {
    Windowed,
    FullscreenLike,
}

impl PresentationClass {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Windowed => "Windowed",
            Self::FullscreenLike => "Fullscreen-like",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureHealth {
    Healthy,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameHealthSample {
    pub dark_pixels: u32,
    pub sampled_pixels: u32,
    pub signature: u64,
}

impl FrameHealthSample {
    pub fn from_bgra(bytes: &[u8], width: u32, height: u32, row_pitch: u32) -> Option<Self> {
        if width == 0 || height == 0 || row_pitch < width.saturating_mul(4) {
            return None;
        }
        let required = usize::try_from(row_pitch)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if bytes.len() < required {
            return None;
        }

        let grid_x = width.min(16);
        let grid_y = height.min(9);
        let mut dark_pixels = 0u32;
        let mut sampled_pixels = 0u32;
        let mut signature = 0xcbf2_9ce4_8422_2325u64;
        for y_index in 0..grid_y {
            let y = ((u64::from(y_index) * u64::from(height)) / u64::from(grid_y)) as u32;
            for x_index in 0..grid_x {
                let x = ((u64::from(x_index) * u64::from(width)) / u64::from(grid_x)) as u32;
                let offset = usize::try_from(y)
                    .ok()?
                    .checked_mul(usize::try_from(row_pitch).ok()?)?
                    .checked_add(usize::try_from(x).ok()?.checked_mul(4)?)?;
                let pixel = bytes.get(offset..offset + 3)?;
                let brightness = u16::from(pixel[0]) + u16::from(pixel[1]) + u16::from(pixel[2]);
                dark_pixels += u32::from(brightness <= 24);
                sampled_pixels += 1;
                for channel in pixel {
                    signature ^= u64::from(*channel);
                    signature = signature.wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        Some(Self {
            dark_pixels,
            sampled_pixels,
            signature,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::FrameHealthSample;

    #[test]
    fn frame_sample_rejects_invalid_stride_and_accepts_a_valid_bgra_frame() {
        assert!(FrameHealthSample::from_bgra(&[], 0, 1, 4).is_none());
        assert!(FrameHealthSample::from_bgra(&[0; 16], 2, 2, 4).is_none());

        let sample = FrameHealthSample::from_bgra(&[0; 16], 2, 2, 8).unwrap();
        assert_eq!(sample.sampled_pixels, 4);
        assert_eq!(sample.dark_pixels, 4);
    }
}
