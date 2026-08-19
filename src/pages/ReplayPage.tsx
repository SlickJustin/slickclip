import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Toggle } from "../components/Toggle";

type CaptureTestResult = {
  success: boolean;
  filePath: string | null;
  errorMessage: string | null;
  borderlessActive: boolean;
  borderlessStatus: string;
  borderedCaptureAvailable: boolean | null;
  requestedEncoder: string;
  actualEncoder: string | null;
};

type ContinuousBaselineResult = {
  success: boolean;
  errorMessage: string | null;
  filePath: string | null;
  requestedEncoder: string;
  actualEncoder: string | null;
  frameRate: number;
  width: number;
  height: number;
  expectedFrameIntervalMs: number;
  totalWallDurationMs: number;
  framesObserved: number;
  firstSourceTimestamp100ns: number | null;
  lastSourceTimestamp100ns: number | null;
  averageConsecutiveDeltaMs: number | null;
  worstConsecutiveDeltaMs: number | null;
  intervalsOverTwoExpected: number;
  estimatedFramesMissed: number;
  averageCallbackDurationMs: number | null;
  worstCallbackDurationMs: number | null;
  averageSendFrameDurationMs: number | null;
  worstSendFrameDurationMs: number | null;
  sendFrameOver16_67Ms: number;
  sendFrameOver33_33Ms: number;
  sendFrameOver50Ms: number;
  sendFrameOver100Ms: number;
  ownedFrameCopies: number;
  averageGpuCopyDurationMs: number | null;
  worstGpuCopyDurationMs: number | null;
  encoderQueueDepth: number;
  maximumEncoderQueueDepth: number;
  encoderQueueCapacity: number;
  encoderQueueFullEvents: number;
  deliberatelyDroppedFrames: number;
  framePoolCreationMethod: string;
  framePoolBufferCount: number;
  finalizationDurationMs: number | null;
};

type EncoderId = "automatic" | "av1" | "hevc" | "h264";

type EncoderInfo = {
  id: EncoderId;
  displayName: string;
  codec: string;
  available: boolean;
  reasonUnavailable: string | null;
  recommended: boolean;
  preferred: boolean;
};

type EncoderCapabilitiesResult = {
  success: boolean;
  encoders: EncoderInfo[];
  automaticEncoderId: EncoderId | null;
  detectionMethod: string;
  hardwareAccelerationRequested: boolean;
  hardwareEncodingVerified: boolean;
  errorMessage: string | null;
};

type MonitorTarget = {
  id: string;
  displayIndex: number;
  friendlyName: string;
  width: number;
  height: number;
  refreshRate: number | null;
  primary: boolean;
};

type WindowTarget = {
  id: string;
  title: string;
  processName: string | null;
  processId: number;
  width: number;
  height: number;
};

type TargetListResult<T> = {
  success: boolean;
  targets: T[];
  errorMessage: string | null;
};

type MicrophoneEndpoint = { id: string; friendlyName: string; state: string; isDefaultMultimedia: boolean; isDefaultCommunications: boolean };
type ApplicationAudioProcess = { processId: number; displayName: string; processName: string; executablePath: string | null };
type AudioListResult<T> = { success: boolean; devices?: T[]; applications?: T[]; error: { message: string } | null };
type AudioTrackRole = "game" | "voiceChat" | "microphone" | "other";
type AudioTrackState = "disabled" | "preparing" | "prepared" | "running" | "ended" | "error" | "stopped";
type AudioFormat = { sampleFormat: string; sampleRate: number; channelCount: number; bitsPerSample: number; validBitsPerSample: number | null };
type AudioTrackStatus = { role: AudioTrackRole; enabled: boolean; state: AudioTrackState; sourceLabel: string | null; format: AudioFormat | null; errorMessage: string | null; retainedDurationSeconds: number; segmentCount: number; totalRetainedBytes: number; packetCount: number; discontinuityCount: number; timestampErrorCount: number; currentQueueDepth: number; maximumQueueDepth: number; queueCapacity: number; queueFullEvents: number; droppedPackets: number; droppedSampleFrames: number; expectedDurationFromSamplesSeconds: number; qpcElapsedDurationSeconds: number | null; sampleQpcDifferenceMs: number | null; estimatedClockDriftPpm: number | null; writerWriteTimeMs: number; writerFinalizeTimeMs: number };
type VideoSegmentPlaybackMap = { sequenceNumber: number; sessionStartQpc100ns: number; sessionEndQpc100ns: number; sourceStartQpc100ns: number; sourceLastFrameQpc100ns: number; encodedStartPts100ns: number; encodedEndPts100ns: number; encodedDuration100ns: number; clipStart100ns: number; clipEnd100ns: number; frameTimingPoints: { frameIndex: number; outputQpc100ns: number; sourceQpc100ns: number; encodedPts100ns: number; freshSource: boolean }[] };
type SavedReplayTimeline = { rawCaptureStartQpc100ns: number; rawCaptureEndQpc100ns: number; rawCaptureSpan100ns: number; clipCaptureStartQpc100ns: number; clipCaptureEndQpc100ns: number; clipPlaybackStart100ns: number; clipPlaybackEnd100ns: number; clipPlaybackDuration100ns: number; encodedTimeBaseNumerator: number; encodedTimeBaseDenominator: number; timestampStrategy: string; segmentMaps: VideoSegmentPlaybackMap[] };
type AudioSnapshotPlan = { trackRole: AudioTrackRole; rawVideoStartQpc100ns: number; rawVideoEndQpc100ns: number; rawVideoSpanMs: number; clipCaptureStartQpc100ns: number; clipCaptureEndQpc100ns: number; clipPlaybackStartMs: number; clipPlaybackEndMs: number; clipPlaybackDurationMs: number; rawAudioStartQpc100ns: number | null; rawAudioEndQpc100ns: number | null; mappedPlaybackStartMs: number | null; mappedPlaybackEndMs: number | null; mappedStartRegion: string | null; mappedEndRegion: string | null; leadingUncoveredMs: number; trailingUncoveredMs: number; trimBeforeClipMs: number; trimAfterClipMs: number; finalClipCoverageMs: number; materialUncoveredThresholdMs: number; hasMaterialUncoveredAudio: boolean; warning: string | null; segmentCount: number; segmentSequenceNumbers: number[] };

type TargetTab = "monitor" | "window";
type SelectedTarget = { targetType: TargetTab; id: string };
type CaptureTestStatus = "idle" | "preparing" | "recording" | "success" | "error";

type ReplayLifecycleState = "stopped" | "starting" | "running" | "stopping" | "error";

type CompletedSegment = {
  sequenceNumber: number;
  filePath: string;
  startTimestampMs: number;
  endTimestampMs: number;
  actualDurationMs: number;
  segmentSessionStartQpc100ns: number;
  segmentSessionEndQpc100ns: number;
  firstFrameTimestamp100ns: number;
  lastFrameTimestamp100ns: number;
  encodedStartPts100ns: number;
  encodedLastFramePts100ns: number;
  encodedEndPts100ns: number;
  encodedDuration100ns: number;
  encodedTimeBaseNumerator: number;
  encodedTimeBaseDenominator: number;
  nextSegmentFirstFrameTimestamp100ns: number | null;
  sourceFrameGapMs: number | null;
  sourceUpdateCount: number;
  freshOutputFrameCount: number;
  heldOutputFrameCount: number;
  frameCount: number;
  encoderCreationTimeMs: number;
  encoderCreationStartedMs: number;
  encoderCreationCompletedMs: number;
  rotationRequestedMs: number | null;
  firstFrameSubmittedMs: number | null;
  lastFrameSubmittedMs: number | null;
  nextFirstFrameSubmittedMs: number | null;
  codec: string;
  width: number;
  height: number;
  frameRate: number;
  fileSize: number;
  averageBitrateMbps: number;
  finalized: boolean;
  finalizationTimeMs: number;
  rotationGapMs: number | null;
};

type ReplayBufferStatus = {
  state: ReplayLifecycleState;
  errorMessage: string | null;
  targetId: string | null;
  targetLabel: string | null;
  requestedEncoder: string | null;
  actualEncoder: string | null;
  replayDurationSeconds: number;
  expectedSegmentDurationSeconds: number;
  frameRate: number;
  width: number;
  height: number;
  sessionId: string | null;
  sessionDirectory: string | null;
  completedSegmentCount: number;
  retainedDurationSeconds: number;
  retainedBytes: number;
  pendingFinalizations: number;
  droppedSegments: number;
  lastSegmentDurationSeconds: number | null;
  lastRotationGapMs: number | null;
  lastFinalizeTimeMs: number | null;
  normalFrameIntervalMs: number | null;
  lastSourceFrameGapMs: number | null;
  worstSourceFrameGapMs: number | null;
  averageSourceFrameGapMs: number | null;
  lastEncoderCreationMs: number | null;
  worstEncoderCreationMs: number | null;
  averageEncoderCreationMs: number | null;
  rotationCount: number;
  framesObserved: number;
  lastEstimatedFramesMissed: number | null;
  estimatedFramesMissedTotal: number;
  materialSourceGapCount: number;
  encoderPreparationInFlight: boolean;
  preparedEncoderReady: boolean;
  nextEncoderState: string;
  averageCallbackDurationMs: number | null;
  worstCallbackDurationMs: number | null;
  averageSendFrameDurationMs: number | null;
  worstSendFrameDurationMs: number | null;
  sendFrameOver16_67Ms: number;
  sendFrameOver33_33Ms: number;
  sendFrameOver50Ms: number;
  sendFrameOver100Ms: number;
  averageCallbackLockWaitMs: number | null;
  worstCallbackLockWaitMs: number | null;
  averageRotationEvaluationMs: number | null;
  worstRotationEvaluationMs: number | null;
  averageSwapDurationMs: number | null;
  worstSwapDurationMs: number | null;
  averageCallbackStateUpdateMs: number | null;
  worstCallbackStateUpdateMs: number | null;
  averageCallbackFilesystemMs: number | null;
  worstCallbackFilesystemMs: number | null;
  callbackFilesystemOperationCount: number;
  ownedFrameCopies: number;
  averageGpuCopyDurationMs: number | null;
  worstGpuCopyDurationMs: number | null;
  encoderQueueDepth: number;
  maximumEncoderQueueDepth: number;
  encoderQueueCapacity: number;
  encoderQueueFullEvents: number;
  deliberatelyDroppedFrames: number;
  videoTimelineStartQpc100ns: number | null;
  schedulerCurrentOutputFrameIndex: number | null;
  schedulerExpectedOutputFrameIndex: number | null;
  schedulerCurrentLatenessMs: number | null;
  schedulerWorstLatenessMs: number | null;
  schedulerCatchUpWakeups: number;
  schedulerMaxCatchUpBurst: number;
  schedulerCatchUpFrames: number;
  schedulerRotationCatchUpFrames: number;
  schedulerSavePendingCatchUpFrames: number;
  queueFullRetryAttempts: number;
  recoveredQueueFullFrames: number;
  lastRotationLatenessBeforeMs: number | null;
  lastRotationLatenessAfterMs: number | null;
  freshOutputFrames: number;
  heldOutputFrames: number;
  supersededSourceUpdates: number;
  missedRealtimeOutputFrames: number;
  sourceFrameUpdateRate: number | null;
  outputCfrRate: number | null;
  framePoolCreationMethod: string;
  framePoolBufferCount: number;
  rotationLifecycle: {
    activeSequenceNumber: number | null;
    nextSequenceNumber: number | null;
    activeSegmentFirstFrameMs: number | null;
    prewarmRequestedMs: number | null;
    encoderCreationStartedMs: number | null;
    encoderCreationCompletedMs: number | null;
    preparedReadyMs: number | null;
    rotationRequestedMs: number | null;
    swapStartedMs: number | null;
    oldSegmentQueuedMs: number | null;
    swapCompletedMs: number | null;
    followingFrameArrivedMs: number | null;
  };
  recentSegments: CompletedSegment[];
  audio: { clock: { sessionStartQpc: number | null; qpcFrequency: number | null; sessionStartQpc100ns: number | null; timingDomain: string }; tracks: AudioTrackStatus[] };
};

type ReplayCommandResult = {
  success: boolean;
  status: ReplayBufferStatus;
  errorMessage: string | null;
};

type SaveJobState =
  | "idle"
  | "preparing"
  | "finalizingCurrentSegment"
  | "assembling"
  | "completed"
  | "error";

type SaveReplayStatus = {
  state: SaveJobState;
  requestedDurationSeconds: number;
  actualSavedDurationSeconds: number | null;
  saveRequestTimestampMs: number | null;
  saveRequestQpc100ns: number | null;
  selectedSegmentCount: number;
  selectedSegmentSequenceNumbers: number[];
  actualEarliestTimestampMs: number | null;
  actualLatestTimestampMs: number | null;
  outputPath: string | null;
  fileSize: number | null;
  codec: string | null;
  errorMessage: string | null;
  audioSnapshotPlans: AudioSnapshotPlan[];
  videoTimeline: SavedReplayTimeline | null;
  internalEncodedDurationSeconds: number | null;
  ffprobeDurationSeconds: number | null;
  internalFfprobeDifferenceMs: number | null;
};

type SaveReplayCommandResult = {
  success: boolean;
  status: SaveReplayStatus;
  errorMessage: string | null;
};

const replayDurationOptions = [
  { label: "30 Seconds", value: 30 },
  { label: "1 Minute", value: 60 },
  { label: "2 Minutes", value: 120 },
  { label: "3 Minutes", value: 180 },
  { label: "5 Minutes", value: 300 },
];

const initialReplayStatus: ReplayBufferStatus = {
  state: "stopped",
  errorMessage: null,
  targetId: null,
  targetLabel: null,
  requestedEncoder: null,
  actualEncoder: null,
  replayDurationSeconds: 0,
  expectedSegmentDurationSeconds: 2,
  frameRate: 0,
  width: 0,
  height: 0,
  sessionId: null,
  sessionDirectory: null,
  completedSegmentCount: 0,
  retainedDurationSeconds: 0,
  retainedBytes: 0,
  pendingFinalizations: 0,
  droppedSegments: 0,
  lastSegmentDurationSeconds: null,
  lastRotationGapMs: null,
  lastFinalizeTimeMs: null,
  normalFrameIntervalMs: null,
  lastSourceFrameGapMs: null,
  worstSourceFrameGapMs: null,
  averageSourceFrameGapMs: null,
  lastEncoderCreationMs: null,
  worstEncoderCreationMs: null,
  averageEncoderCreationMs: null,
  rotationCount: 0,
  framesObserved: 0,
  lastEstimatedFramesMissed: null,
  estimatedFramesMissedTotal: 0,
  materialSourceGapCount: 0,
  encoderPreparationInFlight: false,
  preparedEncoderReady: false,
  nextEncoderState: "not_active",
  averageCallbackDurationMs: null,
  worstCallbackDurationMs: null,
  averageSendFrameDurationMs: null,
  worstSendFrameDurationMs: null,
  sendFrameOver16_67Ms: 0,
  sendFrameOver33_33Ms: 0,
  sendFrameOver50Ms: 0,
  sendFrameOver100Ms: 0,
  averageCallbackLockWaitMs: null,
  worstCallbackLockWaitMs: null,
  averageRotationEvaluationMs: null,
  worstRotationEvaluationMs: null,
  averageSwapDurationMs: null,
  worstSwapDurationMs: null,
  averageCallbackStateUpdateMs: null,
  worstCallbackStateUpdateMs: null,
  averageCallbackFilesystemMs: null,
  worstCallbackFilesystemMs: null,
  callbackFilesystemOperationCount: 0,
  ownedFrameCopies: 0,
  averageGpuCopyDurationMs: null,
  worstGpuCopyDurationMs: null,
  encoderQueueDepth: 0,
  maximumEncoderQueueDepth: 0,
  encoderQueueCapacity: 0,
  encoderQueueFullEvents: 0,
  deliberatelyDroppedFrames: 0,
  videoTimelineStartQpc100ns: null,
  schedulerCurrentOutputFrameIndex: null,
  schedulerExpectedOutputFrameIndex: null,
  schedulerCurrentLatenessMs: null,
  schedulerWorstLatenessMs: null,
  schedulerCatchUpWakeups: 0,
  schedulerMaxCatchUpBurst: 0,
  schedulerCatchUpFrames: 0,
  schedulerRotationCatchUpFrames: 0,
  schedulerSavePendingCatchUpFrames: 0,
  queueFullRetryAttempts: 0,
  recoveredQueueFullFrames: 0,
  lastRotationLatenessBeforeMs: null,
  lastRotationLatenessAfterMs: null,
  freshOutputFrames: 0,
  heldOutputFrames: 0,
  supersededSourceUpdates: 0,
  missedRealtimeOutputFrames: 0,
  sourceFrameUpdateRate: null,
  outputCfrRate: null,
  framePoolCreationMethod: "CreateFreeThreaded",
  framePoolBufferCount: 2,
  rotationLifecycle: {
    activeSequenceNumber: null,
    nextSequenceNumber: null,
    activeSegmentFirstFrameMs: null,
    prewarmRequestedMs: null,
    encoderCreationStartedMs: null,
    encoderCreationCompletedMs: null,
    preparedReadyMs: null,
    rotationRequestedMs: null,
    swapStartedMs: null,
    oldSegmentQueuedMs: null,
    swapCompletedMs: null,
    followingFrameArrivedMs: null,
  },
  recentSegments: [],
  audio: { clock: { sessionStartQpc: null, qpcFrequency: null, sessionStartQpc100ns: null, timingDomain: "" }, tracks: [] },
};

const initialSaveReplayStatus: SaveReplayStatus = {
  state: "idle",
  requestedDurationSeconds: 0,
  actualSavedDurationSeconds: null,
  saveRequestTimestampMs: null,
  saveRequestQpc100ns: null,
  selectedSegmentCount: 0,
  selectedSegmentSequenceNumbers: [],
  actualEarliestTimestampMs: null,
  actualLatestTimestampMs: null,
  outputPath: null,
  fileSize: null,
  codec: null,
  errorMessage: null,
  audioSnapshotPlans: [],
  videoTimeline: null,
  internalEncodedDurationSeconds: null,
  ffprobeDurationSeconds: null,
  internalFfprobeDifferenceMs: null,
};

export function ReplayPage() {
  const [replayDuration, setReplayDuration] = useState(120);
  const [frameRate, setFrameRate] = useState(60);
  const [replayEncoder, setReplayEncoder] = useState<Exclude<EncoderId, "av1">>("automatic");
  const [replayStatus, setReplayStatus] = useState<ReplayBufferStatus>(initialReplayStatus);
  const [replayCommandActive, setReplayCommandActive] = useState(false);
  const [replayCommandError, setReplayCommandError] = useState<string | null>(null);
  const [saveReplayStatus, setSaveReplayStatus] = useState<SaveReplayStatus>(initialSaveReplayStatus);
  const [saveReplayCommandError, setSaveReplayCommandError] = useState<string | null>(null);
  const [captureTestActive, setCaptureTestActive] = useState(false);
  const [captureTestStatus, setCaptureTestStatus] = useState<CaptureTestStatus>("idle");
  const [captureTestResult, setCaptureTestResult] = useState<CaptureTestResult | null>(null);
  const [captureTestMessage, setCaptureTestMessage] = useState(
    "Select a capture target to record a temporary video-only proof.",
  );
  const [baselineActive, setBaselineActive] = useState(false);
  const [baselineResult, setBaselineResult] = useState<ContinuousBaselineResult | null>(null);
  const [targetTab, setTargetTab] = useState<TargetTab>("monitor");
  const [monitors, setMonitors] = useState<MonitorTarget[]>([]);
  const [windows, setWindows] = useState<WindowTarget[]>([]);
  const [selectedTarget, setSelectedTarget] = useState<SelectedTarget | null>(null);
  const [targetsLoading, setTargetsLoading] = useState(true);
  const [targetsError, setTargetsError] = useState<string | null>(null);
  const [captureTestEncoder, setCaptureTestEncoder] = useState<EncoderId>("automatic");
  const [encoderCapabilities, setEncoderCapabilities] = useState<EncoderCapabilitiesResult | null>(null);
  const [encodersLoading, setEncodersLoading] = useState(true);
  const [audioApplications, setAudioApplications] = useState<ApplicationAudioProcess[]>([]);
  const [microphones, setMicrophones] = useState<MicrophoneEndpoint[]>([]);
  const [audioSourcesLoading, setAudioSourcesLoading] = useState(true);
  const [audioSourcesError, setAudioSourcesError] = useState<string | null>(null);
  const [gameEnabled, setGameEnabled] = useState(false);
  const [voiceEnabled, setVoiceEnabled] = useState(false);
  const [microphoneEnabled, setMicrophoneEnabled] = useState(false);
  const [gameProcessId, setGameProcessId] = useState("");
  const [voiceProcessId, setVoiceProcessId] = useState("");
  const [microphoneId, setMicrophoneId] = useState("");
  useEffect(() => {
    void refreshAllTargets();
    void refreshEncoderCapabilities();
    void refreshReplayStatus();
    void refreshSaveReplayStatus();
    void refreshAudioSources();
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen("save-replay-hotkey-feedback", () => {
      setSaveReplayCommandError(null);
      void refreshSaveReplayStatus();
      void refreshReplayStatus();
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!isReplayActive(replayStatus.state)) return;

    const timer = window.setInterval(() => void refreshReplayStatus(), 1_000);
    return () => window.clearInterval(timer);
  }, [replayStatus.state]);

  useEffect(() => {
    if (!isSaveJobActive(saveReplayStatus.state)) return;

    const timer = window.setInterval(() => void refreshSaveReplayStatus(), 750);
    return () => window.clearInterval(timer);
  }, [saveReplayStatus.state]);

  async function refreshEncoderCapabilities() {
    setEncodersLoading(true);

    try {
      const result = await invoke<EncoderCapabilitiesResult>("get_encoder_capabilities");
      setEncoderCapabilities(result);
    } catch (error) {
      setEncoderCapabilities({
        success: false,
        encoders: [],
        automaticEncoderId: null,
        detectionMethod: "Encoder capability detection did not complete.",
        hardwareAccelerationRequested: true,
        hardwareEncodingVerified: false,
        errorMessage: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setEncodersLoading(false);
    }
  }

  async function refreshReplayStatus() {
    try {
      const status = await invoke<ReplayBufferStatus>("get_replay_buffer_status");
      setReplayStatus(status);
      if (status.state === "error") {
        setReplayCommandError(status.errorMessage ?? "The replay buffer entered an unknown error state.");
      }
    } catch (error) {
      setReplayCommandError(error instanceof Error ? error.message : String(error));
    }
  }

  async function refreshSaveReplayStatus() {
    try {
      const status = await invoke<SaveReplayStatus>("get_save_replay_status");
      setSaveReplayStatus(status);
      if (status.state === "error") {
        setSaveReplayCommandError(status.errorMessage ?? "Save Replay failed without an error message.");
      }
    } catch (error) {
      setSaveReplayCommandError(error instanceof Error ? error.message : String(error));
    }
  }

  async function saveReplay() {
    if (isSaveJobActive(saveReplayStatus.state)) return;

    setSaveReplayCommandError(null);
    try {
      const result = await invoke<SaveReplayCommandResult>("save_replay");
      setSaveReplayStatus(result.status);
      if (!result.success) {
        setSaveReplayCommandError(result.errorMessage ?? "Save Replay could not start.");
      }
    } catch (error) {
      setSaveReplayCommandError(error instanceof Error ? error.message : String(error));
      await refreshSaveReplayStatus();
    }
  }

  async function startReplayBuffer() {
    if (!selectedTarget || baselineActive || replayCommandActive || isReplayActive(replayStatus.state)) return;

    setReplayCommandActive(true);
    setReplayCommandError(null);
    try {
      const result = await invoke<ReplayCommandResult>("start_replay_buffer", {
        request: {
          target: selectedTarget,
          encoder: replayEncoder,
          replayDurationSeconds: replayDuration,
          frameRate,
          audio: {
            tracks: [
              ...(gameEnabled ? [{ role: "game", enabled: true, sourceKind: "process", processId: Number(gameProcessId), sourceLabel: audioApplications.find((app) => String(app.processId) === gameProcessId)?.processName ?? null }] : []),
              ...(voiceEnabled ? [{ role: "voiceChat", enabled: true, sourceKind: "process", processId: Number(voiceProcessId), sourceLabel: audioApplications.find((app) => String(app.processId) === voiceProcessId)?.processName ?? null }] : []),
              ...(microphoneEnabled ? [{ role: "microphone", enabled: true, sourceKind: "microphone", endpointId: microphoneId, sourceLabel: microphones.find((mic) => mic.id === microphoneId)?.friendlyName ?? null }] : []),
            ],
          },
        },
      });
      setReplayStatus(result.status);
      if (!result.success) {
        setReplayCommandError(result.errorMessage ?? "The replay buffer could not start.");
      }
    } catch (error) {
      setReplayCommandError(error instanceof Error ? error.message : String(error));
      await refreshReplayStatus();
    } finally {
      setReplayCommandActive(false);
    }
  }

  async function refreshAudioSources() {
    if (isReplayActive(replayStatus.state)) return;
    setAudioSourcesLoading(true);
    setAudioSourcesError(null);
    try {
      const [apps, mics] = await Promise.all([
        invoke<AudioListResult<ApplicationAudioProcess>>("list_application_audio_processes"),
        invoke<AudioListResult<MicrophoneEndpoint>>("list_audio_microphones"),
      ]);
      const nextApps = apps.applications ?? [];
      const nextMics = mics.devices ?? [];
      setAudioApplications(nextApps);
      setMicrophones(nextMics);
      setGameProcessId((value) => nextApps.some((app) => String(app.processId) === value) ? value : "");
      setVoiceProcessId((value) => nextApps.some((app) => String(app.processId) === value) ? value : "");
      setMicrophoneId((value) => nextMics.some((mic) => mic.id === value) ? value : (nextMics[0]?.id ?? ""));
      setAudioSourcesError([apps.error?.message, mics.error?.message].filter(Boolean).join(" ") || null);
    } catch (error) {
      setAudioSourcesError(error instanceof Error ? error.message : String(error));
    } finally {
      setAudioSourcesLoading(false);
    }
  }

  async function stopReplayBuffer() {
    if (replayCommandActive || !isReplayActive(replayStatus.state)) return;

    setReplayCommandActive(true);
    setReplayCommandError(null);
    try {
      const result = await invoke<ReplayCommandResult>("stop_replay_buffer");
      setReplayStatus(result.status);
      if (!result.success) {
        setReplayCommandError(result.errorMessage ?? "The replay buffer did not stop cleanly.");
      }
    } catch (error) {
      setReplayCommandError(error instanceof Error ? error.message : String(error));
      await refreshReplayStatus();
    } finally {
      setReplayCommandActive(false);
    }
  }

  async function refreshAllTargets() {
    setTargetsLoading(true);
    setTargetsError(null);

    try {
      const [monitorResult, windowResult] = await Promise.all([
        invoke<TargetListResult<MonitorTarget>>("list_capture_monitors"),
        invoke<TargetListResult<WindowTarget>>("list_capture_windows"),
      ]);
      setMonitors(monitorResult.targets);
      setWindows(windowResult.targets);
      setTargetsError(
        [monitorResult.errorMessage, windowResult.errorMessage].filter(Boolean).join(" ") || null,
      );
    } catch (error) {
      setTargetsError(error instanceof Error ? error.message : String(error));
    } finally {
      setTargetsLoading(false);
    }
  }

  async function refreshVisibleTargets() {
    setTargetsLoading(true);
    setTargetsError(null);

    try {
      if (targetTab === "monitor") {
        const result = await invoke<TargetListResult<MonitorTarget>>("list_capture_monitors");
        setMonitors(result.targets);
        setTargetsError(result.errorMessage);
        setSelectedTarget((current) =>
          current?.targetType === "monitor" && result.targets.some((target) => target.id === current.id)
            ? current
            : null,
        );
      } else {
        const result = await invoke<TargetListResult<WindowTarget>>("list_capture_windows");
        setWindows(result.targets);
        setTargetsError(result.errorMessage);
        setSelectedTarget((current) =>
          current?.targetType === "window" && result.targets.some((target) => target.id === current.id)
            ? current
            : null,
        );
      }
    } catch (error) {
      setTargetsError(error instanceof Error ? error.message : String(error));
    } finally {
      setTargetsLoading(false);
    }
  }

  function changeTargetTab(tab: TargetTab) {
    if (baselineActive || isReplayActive(replayStatus.state)) return;
    setTargetTab(tab);
    setSelectedTarget(null);
    setTargetsError(null);
  }

  async function recordCaptureTest() {
    if (captureTestActive || baselineActive || !selectedTarget) return;

    setCaptureTestActive(true);
    setCaptureTestResult(null);
    setCaptureTestStatus("preparing");
    setCaptureTestMessage("Checking encoder and borderless capture permission...");

    let unlisten: UnlistenFn | undefined;

    try {
      unlisten = await listen("capture-test-recording-started", () => {
        setCaptureTestStatus("recording");
        setCaptureTestMessage("Recording test...");
      });

      const result = await invoke<CaptureTestResult>("run_capture_test", {
        target: selectedTarget,
        encoder: captureTestEncoder,
      });
      setCaptureTestResult(result);
      if (result.success && result.filePath) {
        setCaptureTestStatus("success");
        setCaptureTestMessage("Capture test completed successfully.");
      } else {
        setCaptureTestStatus("error");
        setCaptureTestMessage(result.errorMessage ?? "Native capture failed without an error message.");
      }
    } catch (error) {
      setCaptureTestStatus("error");
      setCaptureTestMessage(error instanceof Error ? error.message : String(error));
    } finally {
      unlisten?.();
      setCaptureTestActive(false);
    }
  }

  async function runContinuousBaseline() {
    if (!selectedTarget || baselineActive || isReplayActive(replayStatus.state)) return;

    setBaselineActive(true);
    setBaselineResult(null);
    try {
      const result = await invoke<ContinuousBaselineResult>("run_continuous_baseline", {
        target: selectedTarget,
        encoder: replayEncoder,
        frameRate,
      });
      setBaselineResult(result);
    } catch (error) {
      setBaselineResult({
        success: false,
        errorMessage: error instanceof Error ? error.message : String(error),
        filePath: null,
        requestedEncoder: formatEncoderId(replayEncoder),
        actualEncoder: null,
        frameRate,
        width: 0,
        height: 0,
        expectedFrameIntervalMs: 1_000 / frameRate,
        totalWallDurationMs: 0,
        framesObserved: 0,
        firstSourceTimestamp100ns: null,
        lastSourceTimestamp100ns: null,
        averageConsecutiveDeltaMs: null,
        worstConsecutiveDeltaMs: null,
        intervalsOverTwoExpected: 0,
        estimatedFramesMissed: 0,
        averageCallbackDurationMs: null,
        worstCallbackDurationMs: null,
        averageSendFrameDurationMs: null,
        worstSendFrameDurationMs: null,
        sendFrameOver16_67Ms: 0,
        sendFrameOver33_33Ms: 0,
        sendFrameOver50Ms: 0,
        sendFrameOver100Ms: 0,
        ownedFrameCopies: 0,
        averageGpuCopyDurationMs: null,
        worstGpuCopyDurationMs: null,
        encoderQueueDepth: 0,
        maximumEncoderQueueDepth: 0,
        encoderQueueCapacity: 0,
        encoderQueueFullEvents: 0,
        deliberatelyDroppedFrames: 0,
        framePoolCreationMethod: "CreateFreeThreaded",
        framePoolBufferCount: 2,
        finalizationDurationMs: null,
      });
    } finally {
      setBaselineActive(false);
    }
  }

  const selectedEncoderAvailable = encoderCapabilities?.encoders.find(
    (encoderOption) => encoderOption.id === captureTestEncoder,
  )?.available ?? false;

  const replayEncoderAvailable = encoderCapabilities?.encoders.find(
    (encoderOption) => encoderOption.id === replayEncoder,
  )?.available ?? false;
  const replayActive = isReplayActive(replayStatus.state);
  const saveJobActive = isSaveJobActive(saveReplayStatus.state);
  const saveReplayAvailable =
    replayStatus.state === "running" &&
    replayStatus.completedSegmentCount > 0 &&
    !saveJobActive;
  const audioConfigurationValid =
    (!gameEnabled || gameProcessId !== "") &&
    (!voiceEnabled || voiceProcessId !== "") &&
    (!microphoneEnabled || microphoneId !== "") &&
    (!gameEnabled || !voiceEnabled || gameProcessId !== voiceProcessId);
  const selectedTargetLabel = getSelectedTargetLabel(selectedTarget, monitors, windows);

  return (
    <div className="page page-replay">
      <header className="page-header">
        <div>
          <h1>Replay</h1>
          <p>Capture the moments you actually want to keep.</p>
        </div>
        <span className="demo-badge">VIDEO BUFFER</span>
      </header>

      <section className="native-capture-test" aria-labelledby="native-capture-test-heading">
        <div className="native-capture-test-header">
          <div className="native-capture-test-copy">
            <span className="eyebrow">DEVELOPMENT PROOF</span>
            <h2 id="native-capture-test-heading">NATIVE CAPTURE TEST</h2>
            <p>Select a display or application window, then record five seconds of video only.</p>
          </div>
          <button
            className="secondary-button capture-target-refresh"
            type="button"
            disabled={targetsLoading || captureTestActive || replayActive}
            onClick={refreshVisibleTargets}
          >
            {targetsLoading ? "Refreshing..." : "Refresh"}
          </button>
        </div>

        <div className="capture-target-tabs" aria-label="Capture target type">
          <button
            className={targetTab === "monitor" ? "capture-target-tab-active" : ""}
            type="button"
            aria-pressed={targetTab === "monitor"}
            disabled={replayActive}
            onClick={() => changeTargetTab("monitor")}
          >
            Displays <span>{monitors.length}</span>
          </button>
          <button
            className={targetTab === "window" ? "capture-target-tab-active" : ""}
            type="button"
            aria-pressed={targetTab === "window"}
            disabled={replayActive}
            onClick={() => changeTargetTab("window")}
          >
            Windows <span>{windows.length}</span>
          </button>
        </div>

        <div className={`capture-target-list capture-target-list-${targetTab}`}>
          {targetsLoading ? (
            <div className="capture-target-empty">Detecting available {targetTab === "monitor" ? "displays" : "windows"}...</div>
          ) : targetsError ? (
            <div className="capture-target-empty capture-target-load-error">{targetsError}</div>
          ) : targetTab === "monitor" ? (
            monitors.length > 0 ? monitors.map((monitor) => (
              <button
                className={`capture-target-card${selectedTarget?.id === monitor.id ? " capture-target-selected" : ""}`}
                type="button"
                aria-pressed={selectedTarget?.id === monitor.id}
                disabled={replayActive}
                key={monitor.id}
                onClick={() => setSelectedTarget({ targetType: "monitor", id: monitor.id })}
              >
                <span className="capture-target-card-title">
                  Display {monitor.displayIndex}
                  {monitor.primary && <span className="capture-target-primary">Primary</span>}
                </span>
                <span className="capture-target-friendly-name">{monitor.friendlyName}</span>
                <span className="capture-target-details">
                  {monitor.width} × {monitor.height}
                  {monitor.refreshRate && <span>{monitor.refreshRate} Hz</span>}
                </span>
              </button>
            )) : (
              <div className="capture-target-empty">No capturable displays were detected.</div>
            )
          ) : windows.length > 0 ? windows.map((window) => (
            <button
              className={`capture-window-row${selectedTarget?.id === window.id ? " capture-target-selected" : ""}`}
              type="button"
              aria-pressed={selectedTarget?.id === window.id}
              disabled={replayActive}
              key={window.id}
              onClick={() => setSelectedTarget({ targetType: "window", id: window.id })}
            >
              <span className="capture-window-app">{window.processName ?? `Process ${window.processId}`}</span>
              <span className="capture-window-title">{window.title}</span>
              <span className="capture-window-size">{window.width} × {window.height}</span>
            </button>
          )) : (
            <div className="capture-target-empty">No capturable application windows were detected.</div>
          )}
        </div>

        <div className="capture-encoder-section">
          <div className="capture-encoder-heading">
            <div>
              <span className="setting-label">Encoder</span>
              <small>Automatic resolves on the capture backend using AV1 -&gt; HEVC -&gt; H.264 priority.</small>
            </div>
            {encoderCapabilities?.automaticEncoderId && (
              <span className="capture-encoder-auto-result">
                Automatic: {formatEncoderId(encoderCapabilities.automaticEncoderId)}
              </span>
            )}
          </div>

          <div className="capture-encoder-options" aria-label="Test capture encoder">
            {encodersLoading ? (
              <div className="capture-encoder-loading">Probing Windows video encoders...</div>
            ) : encoderCapabilities?.encoders.length ? encoderCapabilities.encoders.map((encoderOption) => (
              <button
                className={`capture-encoder-option${captureTestEncoder === encoderOption.id ? " capture-encoder-selected" : ""}${encoderOption.available ? "" : " capture-encoder-unavailable"}`}
                type="button"
                aria-pressed={captureTestEncoder === encoderOption.id}
                disabled={captureTestActive || replayActive || !encoderOption.available}
                key={encoderOption.id}
                title={encoderOption.reasonUnavailable ?? undefined}
                onClick={() => setCaptureTestEncoder(encoderOption.id)}
              >
                <span className="capture-encoder-name">
                  {encoderOption.displayName}
                  {encoderOption.preferred && <span className="capture-encoder-preferred">Preferred</span>}
                  {encoderOption.recommended && !encoderOption.preferred && <span className="capture-encoder-preferred">Recommended</span>}
                </span>
                <span className={encoderOption.available ? "capture-encoder-available" : "capture-encoder-not-available"}>
                  {encoderOption.available ? "Available" : "Unavailable"}
                </span>
                {encoderOption.reasonUnavailable && <small>{encoderOption.reasonUnavailable}</small>}
              </button>
            )) : (
              <div className="capture-encoder-loading capture-target-load-error">
                {encoderCapabilities?.errorMessage ?? "Encoder capability information is unavailable."}
              </div>
            )}
          </div>

          {encoderCapabilities && (
            <p className="capture-encoder-method">
              {encoderCapabilities.errorMessage ?? `${encoderCapabilities.detectionMethod}. Hardware acceleration is ${encoderCapabilities.hardwareAccelerationRequested ? "requested" : "not requested"}, but hardware encoding is ${encoderCapabilities.hardwareEncodingVerified ? "verified" : "not distinguishable from system/software encoding through this API"}.`}
            </p>
          )}
        </div>

        <div className="native-capture-test-footer">
          <div
            className={`capture-test-result capture-test-${captureTestStatus}`}
            role="status"
            aria-live="polite"
          >
            <span className="capture-test-message">{captureTestMessage}</span>
            {captureTestResult && (
              <div className="capture-test-result-details">
                <span className={captureTestResult.borderlessActive ? "borderless-active" : "borderless-inactive"}>
                  Borderless capture: {captureTestResult.borderlessActive
                    ? "Active"
                    : formatBorderlessStatus(captureTestResult.borderlessStatus)}
                </span>
                {captureTestResult.borderedCaptureAvailable === true && !captureTestResult.borderlessActive && (
                  <span>Normal bordered capture is supported on this system.</span>
                )}
                {captureTestResult.filePath && (
                  <code>{captureTestResult.filePath}</code>
                )}
                {captureTestResult.success && captureTestResult.actualEncoder && (
                  <span className="capture-test-encoder-result">
                    Requested: {captureTestResult.requestedEncoder} / Used: {captureTestResult.actualEncoder}
                  </span>
                )}
              </div>
            )}
          </div>
          <button
            className="primary-button capture-test-button"
            type="button"
            disabled={captureTestActive || baselineActive || replayActive || !selectedTarget || targetsLoading || encodersLoading || !selectedEncoderAvailable}
            onClick={recordCaptureTest}
          >
            {captureTestStatus === "recording" ? "Recording test..." : captureTestActive ? "Preparing capture..." : "Record 5 Second Test"}
          </button>
        </div>
      </section>

      <section className="status-card" aria-labelledby="buffer-heading">
        <div className="status-card-copy">
          <span className="eyebrow">CAPTURE STATUS</span>
          <h2 id="buffer-heading">Replay Buffer</h2>
          <div className={`replay-state replay-state-${replayStatus.state}`}>
            <span className="status-dot" aria-hidden="true" />
            Status: {formatReplayState(replayStatus.state)}
          </div>
          <div className="replay-status-summary">
            <span>Target <strong>{replayStatus.targetLabel ?? selectedTargetLabel ?? "Not selected"}</strong></span>
            <span>Encoder <strong>{replayStatus.actualEncoder ?? "—"}</strong></span>
            <span>Window <strong>{formatDuration(replayStatus.replayDurationSeconds || replayDuration)}</strong></span>
            <span>Retained <strong>{replayStatus.retainedDurationSeconds.toFixed(1)} s</strong></span>
            <span>Segments <strong>{replayStatus.completedSegmentCount}</strong></span>
            <span>Buffer <strong>{formatBytes(replayStatus.retainedBytes)}</strong></span>
          </div>
          {(replayCommandError || replayStatus.errorMessage) && (
            <p className="replay-buffer-error" role="alert">
              {replayCommandError ?? replayStatus.errorMessage}
            </p>
          )}
        </div>
        <button
          className={`primary-button buffer-button${replayActive ? " stop-button" : ""}`}
          type="button"
          aria-pressed={replayActive}
          disabled={
            replayCommandActive ||
            baselineActive ||
            replayStatus.state === "stopping" ||
            (!replayActive && (!selectedTarget || encodersLoading || !replayEncoderAvailable || !audioConfigurationValid))
          }
          onClick={replayActive ? stopReplayBuffer : startReplayBuffer}
        >
          {replayStatus.state === "starting"
            ? "Starting..."
            : replayStatus.state === "stopping"
              ? "Stopping..."
              : replayActive
                ? "Stop Replay Buffer"
                : "Start Replay Buffer"}
        </button>
      </section>

      <div className="replay-grid">
        <div className="replay-config-stack">
          <section className="panel" aria-labelledby="capture-heading">
          <div className="section-heading">
            <div>
              <span className="eyebrow">CONFIGURATION</span>
              <h2 id="capture-heading">Capture</h2>
            </div>
            <span className="section-note">Session only</span>
          </div>

          <div className="setting-row">
            <span className="setting-label">Capture Target</span>
            <span className="replay-setting-value">{selectedTargetLabel ?? "Select a target above"}</span>
          </div>
          <label className="setting-row">
            <span className="setting-label">Replay Duration</span>
            <select
              value={replayDuration}
              disabled={replayActive}
              onChange={(event) => setReplayDuration(Number(event.target.value))}
            >
              {replayDurationOptions.map((option) => (
                <option value={option.value} key={option.value}>{option.label}</option>
              ))}
            </select>
          </label>
          <label className="setting-row">
            <span className="setting-label">Frame Rate</span>
            <select
              value={frameRate}
              disabled={replayActive}
              onChange={(event) => setFrameRate(Number(event.target.value))}
            >
              <option value={30}>30 FPS</option>
              <option value={60}>60 FPS</option>
            </select>
          </label>
          <label className="setting-row">
            <span className="setting-label">Encoder</span>
            <select
              value={replayEncoder}
              disabled={replayActive || encodersLoading}
              onChange={(event) => setReplayEncoder(event.target.value as Exclude<EncoderId, "av1">)}
            >
              <option value="automatic">Automatic</option>
              <option value="hevc" disabled={!isEncoderAvailable(encoderCapabilities, "hevc")}>HEVC</option>
              <option value="h264" disabled={!isEncoderAvailable(encoderCapabilities, "h264")}>H.264</option>
            </select>
          </label>
          <p className="capture-config-note">
            Video remains at the target's native dimensions. Enabled audio sources are retained as independent rolling WAV tracks.
          </p>
          </section>

          <section className="panel replay-audio-panel" aria-labelledby="replay-audio-heading">
          <div className="section-heading">
            <div><span className="eyebrow">INDEPENDENT TRACKS</span><h2 id="replay-audio-heading">Audio</h2></div>
            <button className="secondary-button" type="button" disabled={replayActive || audioSourcesLoading} onClick={() => void refreshAudioSources()}>
              {audioSourcesLoading ? "Refreshing..." : "Refresh Sources"}
            </button>
          </div>
          {audioSourcesError && <p className="replay-buffer-error">{audioSourcesError}</p>}
          <AudioSourceRow label="Game Audio" enabled={gameEnabled} onEnabled={setGameEnabled} locked={replayActive} status={findAudioTrack(replayStatus, "game")}>
            <select value={gameProcessId} disabled={replayActive || !gameEnabled || audioSourcesLoading} onChange={(event) => setGameProcessId(event.target.value)}>
              <option value="">Select application</option>
              {audioApplications.map((app) => <option value={app.processId} key={app.processId}>{app.displayName} ({app.processName}, PID {app.processId})</option>)}
            </select>
          </AudioSourceRow>
          <AudioSourceRow label="Voice Chat" enabled={voiceEnabled} onEnabled={setVoiceEnabled} locked={replayActive} status={findAudioTrack(replayStatus, "voiceChat")}>
            <select value={voiceProcessId} disabled={replayActive || !voiceEnabled || audioSourcesLoading} onChange={(event) => setVoiceProcessId(event.target.value)}>
              <option value="">Select voice-chat application</option>
              {audioApplications.map((app) => <option value={app.processId} key={app.processId}>{app.displayName} ({app.processName}, PID {app.processId})</option>)}
            </select>
          </AudioSourceRow>
          <AudioSourceRow label="Microphone" enabled={microphoneEnabled} onEnabled={setMicrophoneEnabled} locked={replayActive} status={findAudioTrack(replayStatus, "microphone")}>
            <select value={microphoneId} disabled={replayActive || !microphoneEnabled || audioSourcesLoading} onChange={(event) => setMicrophoneId(event.target.value)}>
              <option value="">Select microphone</option>
              {microphones.map((mic) => <option value={mic.id} key={mic.id}>{mic.friendlyName}{mic.isDefaultCommunications ? " (Default communications)" : ""}</option>)}
            </select>
          </AudioSourceRow>
          <div className="replay-audio-source future"><div><strong>Other App</strong><small>Backend role supported; UI assignment is reserved for a later stage.</small></div><span>Disabled</span></div>
          {!audioConfigurationValid && <p className="replay-buffer-error">Choose a source for every enabled track. Game and Voice Chat must use different PIDs.</p>}
          </section>
        </div>

        <div className="replay-side-stack">
          <section className="panel replay-diagnostics" aria-labelledby="diagnostics-heading">
            <div className="section-heading">
              <div>
                <span className="eyebrow">DEVELOPER TELEMETRY</span>
                <h2 id="diagnostics-heading">Segment Diagnostics</h2>
              </div>
            </div>
            <dl className="diagnostic-grid">
              <Diagnostic label="Expected segment" value={`${replayStatus.expectedSegmentDurationSeconds.toFixed(2)} s`} />
              <Diagnostic label="Last segment" value={formatOptionalMetric(replayStatus.lastSegmentDurationSeconds, "s")} />
              <Diagnostic label="Output CFR interval" value={formatOptionalMetric(replayStatus.normalFrameIntervalMs, "ms")} />
              <Diagnostic label="Last WGC delivery gap" value={formatOptionalMetric(replayStatus.lastSourceFrameGapMs, "ms")} />
              <Diagnostic label="Worst WGC delivery gap" value={formatOptionalMetric(replayStatus.worstSourceFrameGapMs, "ms")} />
              <Diagnostic label="Average WGC delivery gap" value={formatOptionalMetric(replayStatus.averageSourceFrameGapMs, "ms")} />
              <Diagnostic label="Last encoder creation" value={formatOptionalMetric(replayStatus.lastEncoderCreationMs, "ms")} />
              <Diagnostic label="Worst encoder creation" value={formatOptionalMetric(replayStatus.worstEncoderCreationMs, "ms")} />
              <Diagnostic label="Average encoder creation" value={formatOptionalMetric(replayStatus.averageEncoderCreationMs, "ms")} />
              <Diagnostic label="Last finalize time" value={formatOptionalMetric(replayStatus.lastFinalizeTimeMs, "ms")} />
              <Diagnostic label="Rotation count" value={String(replayStatus.rotationCount)} />
              <Diagnostic label="WGC source updates" value={String(replayStatus.framesObserved)} />
              <Diagnostic label="Last estimated capture intervals skipped" value={formatOptionalCount(replayStatus.lastEstimatedFramesMissed)} />
              <Diagnostic label="Estimated capture intervals skipped" value={String(replayStatus.estimatedFramesMissedTotal)} />
              <Diagnostic label="Material WGC delivery gaps" value={String(replayStatus.materialSourceGapCount)} />
              <Diagnostic label="Video timeline start QPC" value={formatOptionalCount(replayStatus.videoTimelineStartQpc100ns)} />
              <Diagnostic label="CFR output / expected index" value={`${formatOptionalCount(replayStatus.schedulerCurrentOutputFrameIndex)} / ${formatOptionalCount(replayStatus.schedulerExpectedOutputFrameIndex)}`} />
              <Diagnostic label="Scheduler late current / worst" value={formatMetricPair(replayStatus.schedulerCurrentLatenessMs, replayStatus.schedulerWorstLatenessMs)} />
              <Diagnostic label="Catch-up wakeups / max burst" value={`${replayStatus.schedulerCatchUpWakeups} / ${replayStatus.schedulerMaxCatchUpBurst}`} />
              <Diagnostic label="Scheduler catch-up frames" value={String(replayStatus.schedulerCatchUpFrames)} />
              <Diagnostic label="Catch-up during rotation" value={String(replayStatus.schedulerRotationCatchUpFrames)} />
              <Diagnostic label="Catch-up while Save pending" value={String(replayStatus.schedulerSavePendingCatchUpFrames)} />
              <Diagnostic label="Rotation late before / after" value={formatMetricPair(replayStatus.lastRotationLatenessBeforeMs, replayStatus.lastRotationLatenessAfterMs)} />
              <Diagnostic label="Fresh / held output frames" value={`${replayStatus.freshOutputFrames} / ${replayStatus.heldOutputFrames}`} />
              <Diagnostic label="Superseded WGC updates" value={String(replayStatus.supersededSourceUpdates)} />
              <Diagnostic label="Missed realtime outputs" value={String(replayStatus.missedRealtimeOutputFrames)} />
              <Diagnostic label="Source / output rate" value={`${formatOptionalMetric(replayStatus.sourceFrameUpdateRate, "FPS")} / ${formatOptionalMetric(replayStatus.outputCfrRate, "FPS")}`} />
              <Diagnostic label="Next encoder" value={formatEncoderPreparation(replayStatus)} />
              <Diagnostic label="Callback avg / worst" value={formatMetricPair(replayStatus.averageCallbackDurationMs, replayStatus.worstCallbackDurationMs)} />
              <Diagnostic label="Scheduled submit avg / worst" value={formatMetricPair(replayStatus.averageSendFrameDurationMs, replayStatus.worstSendFrameDurationMs)} />
              <Diagnostic label="Callback lock avg / worst" value={formatMetricPair(replayStatus.averageCallbackLockWaitMs, replayStatus.worstCallbackLockWaitMs)} />
              <Diagnostic label="Rotation eval avg / worst" value={formatMetricPair(replayStatus.averageRotationEvaluationMs, replayStatus.worstRotationEvaluationMs)} />
              <Diagnostic label="Swap avg / worst" value={formatMetricPair(replayStatus.averageSwapDurationMs, replayStatus.worstSwapDurationMs)} />
              <Diagnostic label="State update avg / worst" value={formatMetricPair(replayStatus.averageCallbackStateUpdateMs, replayStatus.worstCallbackStateUpdateMs)} />
              <Diagnostic label="Callback filesystem avg / worst" value={formatMetricPair(replayStatus.averageCallbackFilesystemMs, replayStatus.worstCallbackFilesystemMs)} />
              <Diagnostic label="Callback filesystem ops" value={String(replayStatus.callbackFilesystemOperationCount)} />
              <Diagnostic label="Owned GPU frame copies" value={String(replayStatus.ownedFrameCopies)} />
              <Diagnostic label="GPU copy avg / worst" value={formatMetricPair(replayStatus.averageGpuCopyDurationMs, replayStatus.worstGpuCopyDurationMs)} />
              <Diagnostic label="Encoder queue depth / max" value={`${replayStatus.encoderQueueDepth} / ${replayStatus.maximumEncoderQueueDepth}`} />
              <Diagnostic label="Encoder queue capacity" value={String(replayStatus.encoderQueueCapacity)} />
              <Diagnostic label="Queue refusals / retry attempts" value={`${replayStatus.encoderQueueFullEvents} / ${replayStatus.queueFullRetryAttempts}`} />
              <Diagnostic label="Recovered queue-full frames" value={String(replayStatus.recoveredQueueFullFrames)} />
              <Diagnostic label="WGC frame pool" value={`${replayStatus.framePoolCreationMethod} · ${replayStatus.framePoolBufferCount} buffers`} />
              <Diagnostic label="send_frame > 16.67 / 33.33 ms" value={`${replayStatus.sendFrameOver16_67Ms} / ${replayStatus.sendFrameOver33_33Ms}`} />
              <Diagnostic label="send_frame > 50 / 100 ms" value={`${replayStatus.sendFrameOver50Ms} / ${replayStatus.sendFrameOver100Ms}`} />
              <Diagnostic label="Pending finalizations" value={String(replayStatus.pendingFinalizations)} />
              <Diagnostic label="Dropped segments" value={String(replayStatus.droppedSegments)} />
              <Diagnostic label="Video format" value={replayStatus.width ? `${replayStatus.width} × ${replayStatus.height} @ ${replayStatus.frameRate} FPS` : "—"} />
              <Diagnostic label="Session" value={replayStatus.sessionId ?? "—"} />
            </dl>
            <div className="rotation-lifecycle">
              <span className="setting-label">Current rotation lifecycle (session-relative ms)</span>
              <code>{formatRotationLifecycle(replayStatus)}</code>
            </div>
            <div className="continuous-baseline">
              <button
                className="secondary-button"
                type="button"
                disabled={
                  baselineActive ||
                  captureTestActive ||
                  replayActive ||
                  !selectedTarget ||
                  encodersLoading ||
                  !replayEncoderAvailable
                }
                onClick={runContinuousBaseline}
              >
                {baselineActive ? "Running 20 Second Baseline..." : "Run 20 Second Continuous Baseline"}
              </button>
              <small>
                Uses the selected target, {frameRate} FPS, and the Replay Buffer encoder. No rotations,
                prewarming, FFmpeg, or save work runs during capture.
              </small>
              {baselineResult && (
                <div className={`baseline-result ${baselineResult.success ? "baseline-result-success" : "baseline-result-error"}`} role="status">
                  <strong>{baselineResult.success ? "Continuous baseline completed" : "Continuous baseline failed"}</strong>
                  {baselineResult.errorMessage && <span>{baselineResult.errorMessage}</span>}
                  {baselineResult.success && (
                    <>
                      <dl className="diagnostic-grid baseline-diagnostic-grid">
                        <Diagnostic label="Wall duration" value={`${(baselineResult.totalWallDurationMs / 1_000).toFixed(2)} s`} />
                        <Diagnostic label="Frames observed" value={String(baselineResult.framesObserved)} />
                        <Diagnostic label="Expected interval" value={`${baselineResult.expectedFrameIntervalMs.toFixed(2)} ms`} />
                        <Diagnostic label="Delta avg / worst" value={formatMetricPair(baselineResult.averageConsecutiveDeltaMs, baselineResult.worstConsecutiveDeltaMs)} />
                        <Diagnostic label="Intervals > 2× expected" value={String(baselineResult.intervalsOverTwoExpected)} />
                        <Diagnostic label="Estimated missed" value={String(baselineResult.estimatedFramesMissed)} />
                        <Diagnostic label="Callback avg / worst" value={formatMetricPair(baselineResult.averageCallbackDurationMs, baselineResult.worstCallbackDurationMs)} />
                        <Diagnostic label="send_frame avg / worst" value={formatMetricPair(baselineResult.averageSendFrameDurationMs, baselineResult.worstSendFrameDurationMs)} />
                        <Diagnostic label="send_frame > 16.67 / 33.33 ms" value={`${baselineResult.sendFrameOver16_67Ms} / ${baselineResult.sendFrameOver33_33Ms}`} />
                        <Diagnostic label="send_frame > 50 / 100 ms" value={`${baselineResult.sendFrameOver50Ms} / ${baselineResult.sendFrameOver100Ms}`} />
                        <Diagnostic label="Owned GPU frame copies" value={String(baselineResult.ownedFrameCopies)} />
                        <Diagnostic label="GPU copy avg / worst" value={formatMetricPair(baselineResult.averageGpuCopyDurationMs, baselineResult.worstGpuCopyDurationMs)} />
                        <Diagnostic label="Encoder queue depth / max" value={`${baselineResult.encoderQueueDepth} / ${baselineResult.maximumEncoderQueueDepth}`} />
                        <Diagnostic label="Encoder queue capacity" value={String(baselineResult.encoderQueueCapacity)} />
                        <Diagnostic label="Queue-full / dropped frames" value={`${baselineResult.encoderQueueFullEvents} / ${baselineResult.deliberatelyDroppedFrames}`} />
                        <Diagnostic label="WGC frame pool" value={`${baselineResult.framePoolCreationMethod} · ${baselineResult.framePoolBufferCount} buffers`} />
                        <Diagnostic label="First source timestamp" value={formatOptionalCount(baselineResult.firstSourceTimestamp100ns)} />
                        <Diagnostic label="Last source timestamp" value={formatOptionalCount(baselineResult.lastSourceTimestamp100ns)} />
                        <Diagnostic label="Finalize after capture" value={formatOptionalMetric(baselineResult.finalizationDurationMs, "ms")} />
                        <Diagnostic label="Video format" value={`${baselineResult.width} × ${baselineResult.height} @ ${baselineResult.frameRate} FPS`} />
                      </dl>
                      {baselineResult.filePath && <code className="baseline-output-path">{baselineResult.filePath}</code>}
                    </>
                  )}
                </div>
              )}
            </div>
            <div className="recent-segments">
              <span className="setting-label">Recent finalized segments</span>
              {replayStatus.recentSegments.length ? (
                replayStatus.recentSegments.map((segment) => (
                  <div className="recent-segment-row" key={segment.sequenceNumber}>
                    <code>#{String(segment.sequenceNumber).padStart(6, "0")}</code>
                    <span>{(segment.actualDurationMs / 1_000).toFixed(2)} s</span>
                    <span>{segment.frameCount} frames</span>
                    <span>{segment.freshOutputFrameCount} fresh / {segment.heldOutputFrameCount} held</span>
                    <span>{segment.sourceFrameGapMs === null ? "final" : `${segment.sourceFrameGapMs.toFixed(2)} ms source gap`}</span>
                    <span>{segment.encoderCreationTimeMs.toFixed(2)} ms create</span>
                    <span>{formatBytes(segment.fileSize)} / {segment.averageBitrateMbps.toFixed(2)} Mbps</span>
                  </div>
                ))
              ) : (
                <p>No finalized segments yet.</p>
              )}
            </div>
            {replayStatus.audio.tracks.length > 0 && (
              <div className="recent-segments">
                <span className="setting-label">Audio track telemetry</span>
                {replayStatus.audio.tracks.map((track) => <AudioTrackTelemetry track={track} key={track.role} />)}
                <code>{replayStatus.audio.clock.timingDomain}</code>
              </div>
            )}
          </section>

          <section className="panel save-panel" aria-labelledby="save-heading">
            <div className="section-heading">
              <div>
                <span className="eyebrow">MANUAL CAPTURE</span>
                <h2 id="save-heading">Save Replay</h2>
              </div>
            </div>
            <div
              className={`save-replay-status save-replay-status-${saveReplayStatus.state}`}
              role="status"
              aria-live="polite"
            >
              <strong>{formatSaveJobMessage(saveReplayStatus.state)}</strong>
              {(saveReplayCommandError || saveReplayStatus.errorMessage) && (
                <span className="save-replay-error">
                  {saveReplayCommandError ?? saveReplayStatus.errorMessage}
                </span>
              )}
              {saveReplayStatus.outputPath && (
                <code>{saveReplayStatus.outputPath}</code>
              )}
              {saveReplayStatus.actualSavedDurationSeconds !== null && (
                <div className="saved-replay-details">
                  <span>{formatReplayClipDuration(saveReplayStatus.actualSavedDurationSeconds)}</span>
                  <span>{saveReplayStatus.selectedSegmentCount} segments</span>
                  <span>{saveReplayStatus.codec ?? "Unknown codec"}</span>
                  {saveReplayStatus.fileSize !== null && (
                    <span>
                      {formatBytes(saveReplayStatus.fileSize)} / {formatAverageBitrate(
                        saveReplayStatus.fileSize,
                        saveReplayStatus.actualSavedDurationSeconds,
                      )}
                    </span>
                  )}
                </div>
              )}
              {saveReplayStatus.audioSnapshotPlans.length > 0 && (
                <div className="audio-snapshot-plans">
                  <small>Audio selection plan (diagnostic only; saved MP4 remains video-only)</small>
                  {saveReplayStatus.audioSnapshotPlans.map((plan) => (
                    <div className={`audio-snapshot-plan${plan.hasMaterialUncoveredAudio ? " warning" : ""}`} key={plan.trackRole}>
                      <strong>{formatAudioRole(plan.trackRole)}</strong>
                      <code>Raw audio QPC {plan.rawAudioStartQpc100ns ?? "—"}→{plan.rawAudioEndQpc100ns ?? "—"}</code>
                      <code>Mapped playback {formatOptionalMetric(plan.mappedPlaybackStartMs, "ms")}→{formatOptionalMetric(plan.mappedPlaybackEndMs, "ms")}</code>
                      <span>Coverage {plan.finalClipCoverageMs.toFixed(3)} ms · leading/trailing uncovered {plan.leadingUncoveredMs.toFixed(3)} / {plan.trailingUncoveredMs.toFixed(3)} ms</span>
                      <span>Trim before/after {plan.trimBeforeClipMs.toFixed(3)} / {plan.trimAfterClipMs.toFixed(3)} ms · {plan.segmentCount} segments</span>
                      <code>Video QPC anchor {plan.clipCaptureStartQpc100ns}→{plan.clipCaptureEndQpc100ns}</code>
                      {plan.warning && <span className="save-replay-error">{plan.warning}</span>}
                    </div>
                  ))}
                </div>
              )}
              {saveReplayStatus.videoTimeline && (
                <div className="timeline-consistency-report">
                  <small>Saved-replay timeline consistency</small>
                  <code>Immutable Save QPC anchor {saveReplayStatus.saveRequestQpc100ns ?? "n/a"}</code>
                  <code>Raw WGC QPC {saveReplayStatus.videoTimeline.rawCaptureStartQpc100ns}→{saveReplayStatus.videoTimeline.rawCaptureEndQpc100ns} ({format100nsSeconds(saveReplayStatus.videoTimeline.rawCaptureSpan100ns)} s)</code>
                  <code>Realtime video QPC {saveReplayStatus.videoTimeline.clipCaptureStartQpc100ns}→{saveReplayStatus.videoTimeline.clipCaptureEndQpc100ns}</code>
                  <code>Final playback 0.000→{format100nsSeconds(saveReplayStatus.videoTimeline.clipPlaybackDuration100ns)} s · source delivery gaps preserved by held CFR frames</code>
                  <code>Internal / ffprobe {formatOptionalMetric(saveReplayStatus.internalEncodedDurationSeconds, "s")} / {formatOptionalMetric(saveReplayStatus.ffprobeDurationSeconds, "s")} · difference {formatOptionalMetric(saveReplayStatus.internalFfprobeDifferenceMs, "ms")}</code>
                  <small>{saveReplayStatus.videoTimeline.timestampStrategy}</small>
                </div>
              )}
              {saveReplayStatus.state === "completed" &&
                saveReplayStatus.actualSavedDurationSeconds !== null &&
                saveReplayStatus.actualSavedDurationSeconds + 0.05 < saveReplayStatus.requestedDurationSeconds && (
                  <small>
                    The buffer had less than {formatDuration(saveReplayStatus.requestedDurationSeconds)} available.
                  </small>
                )}
            </div>
            <button
              className="save-replay-button"
              type="button"
              disabled={!saveReplayAvailable}
              title={saveReplayAvailable ? undefined : saveReplayDisabledReason(replayStatus, saveReplayStatus)}
              onClick={saveReplay}
            >
              Save Replay
            </button>
            {!saveReplayAvailable && !saveJobActive && saveReplayStatus.state !== "completed" && (
              <span className="disabled-reason">{saveReplayDisabledReason(replayStatus, saveReplayStatus)}</span>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}

function formatBorderlessStatus(status: string) {
  const labels: Record<string, string> = {
    capability_not_declared: "Required capability not declared",
    denied_by_system: "Denied by Windows",
    denied_by_user: "Denied by user",
    permission_check_failed: "Support check failed",
    permission_request_failed: "Permission request failed",
    user_prompt_required: "User consent still required",
    unsupported: "Unsupported by this Windows version",
    permission_granted: "Permission granted; capture failed later",
    not_attempted: "Not active",
  };

  return labels[status] ?? status.split("_").join(" ");
}

function formatEncoderId(encoder: EncoderId) {
  const labels: Record<EncoderId, string> = {
    automatic: "Automatic",
    av1: "AV1",
    hevc: "HEVC",
    h264: "H.264",
  };

  return labels[encoder];
}

function isReplayActive(state: ReplayLifecycleState) {
  return state === "starting" || state === "running" || state === "stopping";
}

function isSaveJobActive(state: SaveJobState) {
  return state === "preparing" || state === "finalizingCurrentSegment" || state === "assembling";
}

function formatSaveJobMessage(state: SaveJobState) {
  const messages: Record<SaveJobState, string> = {
    idle: "Ready to save available replay video.",
    preparing: "Preparing replay...",
    finalizingCurrentSegment: "Waiting for the next prewarmed segment boundary...",
    assembling: "Saving replay...",
    completed: "Replay saved",
    error: "Replay save failed",
  };
  return messages[state];
}

function saveReplayDisabledReason(
  replay: ReplayBufferStatus,
  save: SaveReplayStatus,
) {
  if (isSaveJobActive(save.state)) return "A replay is already being saved.";
  if (replay.state !== "running") return "Start the Replay Buffer before saving.";
  if (replay.completedSegmentCount === 0) return "Waiting for the first finalized segment.";
  return "Save Replay is temporarily unavailable.";
}

function formatReplayClipDuration(seconds: number) {
  if (seconds < 60) return `${seconds.toFixed(1)} second replay`;
  const totalSeconds = Math.round(seconds);
  const minutes = Math.floor(totalSeconds / 60);
  const remainder = totalSeconds % 60;
  return `${minutes}:${String(remainder).padStart(2, "0")} replay`;
}

function isEncoderAvailable(capabilities: EncoderCapabilitiesResult | null, id: EncoderId) {
  return capabilities?.encoders.some((encoder) => encoder.id === id && encoder.available) ?? false;
}

function getSelectedTargetLabel(
  selected: SelectedTarget | null,
  monitors: MonitorTarget[],
  windows: WindowTarget[],
) {
  if (!selected) return null;
  if (selected.targetType === "monitor") {
    const monitor = monitors.find((target) => target.id === selected.id);
    return monitor ? `Display ${monitor.displayIndex} - ${monitor.friendlyName}` : "Selected display";
  }

  const window = windows.find((target) => target.id === selected.id);
  return window ? `${window.processName ?? `Process ${window.processId}`} - ${window.title}` : "Selected window";
}

function formatReplayState(state: ReplayLifecycleState) {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

function formatDuration(seconds: number) {
  if (seconds < 60) return `${seconds} seconds`;
  const minutes = seconds / 60;
  return `${minutes} minute${minutes === 1 ? "" : "s"}`;
}

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  if (bytes < 1_073_741_824) return `${(bytes / 1_048_576).toFixed(1)} MB`;
  return `${(bytes / 1_073_741_824).toFixed(2)} GB`;
}

function formatAverageBitrate(bytes: number, durationSeconds: number) {
  if (durationSeconds <= 0) return "n/a";
  return `${(bytes * 8 / durationSeconds / 1_000_000).toFixed(2)} Mbps`;
}

function format100nsSeconds(value: number) {
  return (value / 10_000_000).toFixed(6);
}

function formatOptionalMetric(value: number | null, unit: string) {
  return value === null ? "—" : `${value.toFixed(2)} ${unit}`;
}

function formatOptionalCount(value: number | null) {
  return value === null ? "—" : String(value);
}

function formatEncoderPreparation(status: ReplayBufferStatus) {
  const labels: Record<string, string> = {
    not_active: "Not active",
    starting: "Capture starting",
    stopping: "Capture stopping",
    waiting_for_prewarm_point: "Waiting for prewarm point",
    preparing: "Preparing",
    ready: "Ready",
    rotation_due_waiting_for_encoder: "Rotation due; waiting for encoder",
    error: "Unavailable after error",
  };
  return labels[status.nextEncoderState] ?? status.nextEncoderState.split("_").join(" ");
}

function formatMetricPair(average: number | null, worst: number | null) {
  if (average === null || worst === null) return "—";
  return `${average.toFixed(3)} / ${worst.toFixed(3)} ms`;
}

function formatRotationLifecycle(status: ReplayBufferStatus) {
  const trace = status.rotationLifecycle;
  const metric = (value: number | null) => value === null ? "—" : value.toFixed(2);
  return [
    `active #${trace.activeSequenceNumber ?? "—"}`,
    `first ${metric(trace.activeSegmentFirstFrameMs)}`,
    `next #${trace.nextSequenceNumber ?? "—"}`,
    `prewarm ${metric(trace.prewarmRequestedMs)}`,
    `create ${metric(trace.encoderCreationStartedMs)}→${metric(trace.encoderCreationCompletedMs)}`,
    `ready ${metric(trace.preparedReadyMs)}`,
    `due ${metric(trace.rotationRequestedMs)}`,
    `swap ${metric(trace.swapStartedMs)}→${metric(trace.swapCompletedMs)}`,
    `queued ${metric(trace.oldSegmentQueuedMs)}`,
    `following ${metric(trace.followingFrameArrivedMs)}`,
  ].join(" · ");
}

function Diagnostic({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function findAudioTrack(status: ReplayBufferStatus, role: AudioTrackRole) {
  return status.audio.tracks.find((track) => track.role === role) ?? null;
}

function formatAudioRole(role: AudioTrackRole) {
  return ({ game: "Game", voiceChat: "Voice Chat", microphone: "Microphone", other: "Other" } as const)[role];
}

function formatAudioFormat(format: AudioFormat | null) {
  if (!format) return "Format pending";
  const channels = format.channelCount === 1 ? "Mono" : format.channelCount === 2 ? "Stereo" : `${format.channelCount} channels`;
  return `${format.sampleRate / 1_000} kHz ${channels} ${format.sampleFormat}`;
}

function AudioSourceRow({ label, enabled, onEnabled, locked, status, children }: { label: string; enabled: boolean; onEnabled: (value: boolean) => void; locked: boolean; status: AudioTrackStatus | null; children: React.ReactNode }) {
  return (
    <div className="replay-audio-source">
      <div className="replay-audio-source-heading"><div><strong>{label}</strong>{status && <small>{status.state}: {status.sourceLabel ?? "source pending"}</small>}</div><Toggle label={`Enable ${label}`} checked={enabled} onChange={onEnabled} disabled={locked} /></div>
      {children}
      {status?.format && <small>{formatAudioFormat(status.format)} · Retained {status.retainedDurationSeconds.toFixed(1)} s · Drops {status.droppedPackets}</small>}
      {status?.errorMessage && <small className="replay-buffer-error">{status.errorMessage}</small>}
    </div>
  );
}

function AudioTrackTelemetry({ track }: { track: AudioTrackStatus }) {
  return (
    <div className="recent-segment-row audio-track-row">
      <strong>{formatAudioRole(track.role)} · {track.state}</strong>
      <span>{track.sourceLabel ?? "—"}</span>
      <span>{formatAudioFormat(track.format)}</span>
      <span>{track.retainedDurationSeconds.toFixed(1)} s / {track.segmentCount} segments / {formatBytes(track.totalRetainedBytes)}</span>
      <span>queue {track.currentQueueDepth}/{track.maximumQueueDepth}/{track.queueCapacity} · full {track.queueFullEvents}</span>
      <span>drops {track.droppedPackets} packets / {track.droppedSampleFrames} frames</span>
      <span>disc {track.discontinuityCount} · timestamp {track.timestampErrorCount}</span>
      <span>writer write/finalize {track.writerWriteTimeMs.toFixed(2)} / {track.writerFinalizeTimeMs.toFixed(2)} ms</span>
      <span>sample−QPC {formatOptionalMetric(track.sampleQpcDifferenceMs, "ms")} · drift {formatOptionalMetric(track.estimatedClockDriftPpm, "ppm")}</span>
    </div>
  );
}
