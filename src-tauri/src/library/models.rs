use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: i64 = 1;
pub const CLIP_METADATA_VERSION: i64 = 1;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipAudioTrack {
    pub stream_index: u32,
    pub role: String,
    pub title: Option<String>,
    pub handler_name: Option<String>,
    pub codec: String,
    pub profile: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u16>,
    pub bitrate_bps: Option<u64>,
    pub is_default: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipListItem {
    pub id: String,
    pub file_path: String,
    pub filename: String,
    pub display_name: String,
    pub created_at_ms: i64,
    pub library_added_at_ms: i64,
    pub file_modified_at_ms: i64,
    pub file_size_bytes: u64,
    pub duration_100ns: i64,
    pub requested_duration_seconds: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub video_codec: String,
    pub video_profile: Option<String>,
    pub video_bitrate_bps: Option<u64>,
    pub total_bitrate_bps: Option<u64>,
    pub capture_target_label: Option<String>,
    pub capture_target_type: Option<String>,
    pub favorite: bool,
    pub imported_existing_file: bool,
    pub audio_stream_count: u32,
    pub default_audio_stream_title: Option<String>,
    pub metadata_version: i64,
    pub audio_tracks: Vec<ClipAudioTrack>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipSortOrder {
    #[default]
    NewestFirst,
    OldestFirst,
    NameAscending,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipListRequest {
    #[serde(default)]
    pub search_text: Option<String>,
    #[serde(default)]
    pub favorites_only: bool,
    #[serde(default)]
    pub sort_order: ClipSortOrder,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_limit() -> u32 {
    100
}

impl ClipListRequest {
    pub fn normalized(mut self) -> Self {
        self.limit = self.limit.clamp(1, 200);
        self.offset = self.offset.min(1_000_000);
        self.search_text = self
            .search_text
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipIdRequest {
    pub clip_id: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetFavoriteRequest {
    pub clip_id: String,
    pub favorite: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameClipRequest {
    pub clip_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationTelemetry {
    pub scanned_files: u64,
    pub unchanged: u64,
    pub added: u64,
    pub updated: u64,
    pub removed: u64,
    pub failed: u64,
    pub duration_ms: f64,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryTelemetry {
    pub database_path: String,
    pub schema_version: i64,
    pub indexed_clip_count: u64,
    pub reconciliation_running: bool,
    pub last_reconciliation: Option<ReconciliationTelemetry>,
    pub last_list_query_duration_ms: Option<f64>,
    pub newest_saved_clip_id: Option<String>,
    pub newest_saved_clip_indexed: Option<bool>,
    pub newest_saved_clip_insertion_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipListResponse {
    pub success: bool,
    pub clips: Vec<ClipListItem>,
    pub total_count: u64,
    pub telemetry: LibraryTelemetry,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconcileResponse {
    pub success: bool,
    pub result: Option<ReconciliationTelemetry>,
    pub telemetry: LibraryTelemetry,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipMutationResponse {
    pub success: bool,
    pub clip: Option<ClipListItem>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipActionResponse {
    pub success: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CacheArtifactState {
    #[default]
    Missing,
    Preparing,
    Ready,
    Error,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheArtifactStatus {
    pub state: CacheArtifactState,
    pub file_path: Option<String>,
    pub generation_duration_ms: Option<f64>,
    pub file_size_bytes: Option<u64>,
    pub bitrate_bps: Option<u64>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPlaybackInfo {
    pub clip_id: String,
    pub display_name: String,
    pub master_path: String,
    pub master_codec: String,
    pub width: u32,
    pub height: u32,
    pub duration_100ns: i64,
    pub audio_tracks: Vec<ClipAudioTrack>,
    pub cache_root: String,
    pub preview: CacheArtifactStatus,
    pub thumbnail: CacheArtifactStatus,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipPlaybackInfoResponse {
    pub success: bool,
    pub info: Option<ClipPlaybackInfo>,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareClipMediaRequest {
    pub clip_id: String,
    #[serde(default)]
    pub retry: bool,
    #[serde(default)]
    pub current_time_seconds: f64,
    #[serde(default)]
    pub was_playing: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareClipAudioRequest {
    pub clip_id: String,
    pub stream_index: u32,
    #[serde(default)]
    pub retry: bool,
    #[serde(default)]
    pub current_time_seconds: f64,
    #[serde(default)]
    pub was_playing: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepareClipMediaResponse {
    pub success: bool,
    pub artifact: CacheArtifactStatus,
    pub playback_source: Option<String>,
    pub selected_audio_role: Option<String>,
    pub restore_at_seconds: f64,
    pub resume_playing: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClipUpsert {
    pub id: String,
    pub file_path: String,
    pub filename: String,
    pub display_name: String,
    pub created_at_ms: i64,
    pub library_added_at_ms: i64,
    pub file_modified_at_ms: i64,
    pub file_size_bytes: u64,
    pub duration_100ns: i64,
    pub requested_duration_seconds: Option<u32>,
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub video_codec: String,
    pub video_profile: Option<String>,
    pub video_bitrate_bps: Option<u64>,
    pub total_bitrate_bps: Option<u64>,
    pub capture_target_label: Option<String>,
    pub capture_target_type: Option<String>,
    pub imported_existing_file: bool,
    pub audio_tracks: Vec<ClipAudioTrack>,
}

#[derive(Clone, Debug)]
pub struct ClipFingerprint {
    pub id: String,
    pub file_path: String,
    pub file_size_bytes: u64,
    pub file_modified_at_ms: i64,
}
