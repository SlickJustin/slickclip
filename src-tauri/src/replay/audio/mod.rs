mod buffer;
mod clock;
mod segment;
mod track;

use serde::{Deserialize, Serialize};

pub use buffer::{
    AudioReplayShared, AudioReplayStatus, AudioSaveBarrierTelemetry, AudioSnapshotPinGuard,
    AudioSnapshotPlan, AudioSnapshotTrack,
};
pub use clock::ReplaySessionClock;
#[cfg(test)]
pub use segment::CompletedAudioSegment;
pub use track::AudioReplaySession;

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq, Ord, PartialOrd, Hash,
)]
#[serde(rename_all = "camelCase")]
pub enum AudioTrackRole {
    #[default]
    Game,
    VoiceChat,
    Microphone,
    Other,
}

impl AudioTrackRole {
    pub const fn directory_name(self) -> &'static str {
        match self {
            Self::Game => "game",
            Self::VoiceChat => "voice-chat",
            Self::Microphone => "microphone",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AudioSourceKind {
    #[default]
    Process,
    Microphone,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AudioTrackState {
    #[default]
    Disabled,
    Preparing,
    Prepared,
    Running,
    Ended,
    Error,
    Stopped,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackConfiguration {
    pub role: AudioTrackRole,
    pub enabled: bool,
    pub source_kind: AudioSourceKind,
    #[serde(default)]
    pub process_id: Option<u32>,
    #[serde(default)]
    pub endpoint_id: Option<String>,
    #[serde(default)]
    pub source_label: Option<String>,
}

impl AudioTrackConfiguration {
    pub fn source_identifier(&self) -> Option<String> {
        match self.source_kind {
            AudioSourceKind::Process => self.process_id.map(|pid| pid.to_string()),
            AudioSourceKind::Microphone => self.endpoint_id.clone(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        match (self.role, self.source_kind) {
            (AudioTrackRole::Microphone, AudioSourceKind::Microphone)
                if self
                    .endpoint_id
                    .as_ref()
                    .is_some_and(|id| !id.trim().is_empty()) =>
            {
                Ok(())
            }
            (AudioTrackRole::Microphone, _) => {
                Err("The Microphone role requires a stable endpoint ID.".into())
            }
            (_, AudioSourceKind::Process) if self.process_id.is_some_and(|pid| pid > 0) => Ok(()),
            (_, AudioSourceKind::Microphone) => Err(format!(
                "{:?} requires an application PID, not a microphone endpoint.",
                self.role
            )),
            _ => Err(format!("{:?} requires a valid application PID.", self.role)),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioReplayConfiguration {
    #[serde(default)]
    pub tracks: Vec<AudioTrackConfiguration>,
}

impl AudioReplayConfiguration {
    pub fn validate(&self) -> Result<(), String> {
        let mut roles = std::collections::BTreeSet::new();
        let mut sources = std::collections::BTreeSet::new();
        for track in &self.tracks {
            track.validate()?;
            if !roles.insert(track.role) {
                return Err(format!(
                    "Audio role {:?} is configured more than once.",
                    track.role
                ));
            }
            if track.enabled {
                let key = (
                    track.source_kind as u8,
                    track.source_identifier().unwrap_or_default(),
                );
                if !sources.insert(key) {
                    return Err(
                        "The same audio source cannot have two workers in one Replay session."
                            .into(),
                    );
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_names_are_stable_and_configuration_requires_ids() {
        assert_eq!(AudioTrackRole::VoiceChat.directory_name(), "voice-chat");
        let missing = AudioTrackConfiguration {
            role: AudioTrackRole::Microphone,
            enabled: true,
            source_kind: AudioSourceKind::Microphone,
            process_id: None,
            endpoint_id: None,
            source_label: None,
        };
        assert!(missing.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_roles_and_sources() {
        let track = AudioTrackConfiguration {
            role: AudioTrackRole::Game,
            enabled: true,
            source_kind: AudioSourceKind::Process,
            process_id: Some(42),
            endpoint_id: None,
            source_label: None,
        };
        assert!(AudioReplayConfiguration {
            tracks: vec![track.clone(), track]
        }
        .validate()
        .is_err());
    }
}
