import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useRef, useState } from "react";

const TARGET_SAMPLE_RATE = 16000;
const CHUNK_SAMPLES = 3200;
const BYTE_CHUNK_SIZE = 0x8000;

interface DispatcherAsrEvent {
  workspaceId: string;
  kind: "started" | "partial" | "final" | "finished" | "error" | "cancelled";
  text?: string;
  message?: string;
}

interface UseDashScopeAsrOptions {
  workspaceId: string;
  enabled: boolean;
  onTranscriptReady: (text: string) => Promise<void> | void;
}

interface UseDashScopeAsrResult {
  isRecording: boolean;
  transcript: string;
  error: string | null;
  startRecording: () => Promise<void>;
  stopRecording: () => Promise<void>;
  toggleRecording: () => Promise<void>;
  clearError: () => void;
}

function clampToInt16(sample: number): number {
  const normalized = Math.max(-1, Math.min(1, sample));
  return normalized < 0 ? normalized * 0x8000 : normalized * 0x7fff;
}

function downsampleTo16k(input: Float32Array, inputRate: number): Int16Array {
  if (input.length === 0) {
    return new Int16Array(0);
  }

  if (inputRate === TARGET_SAMPLE_RATE) {
    const direct = new Int16Array(input.length);
    for (let i = 0; i < input.length; i += 1) {
      direct[i] = clampToInt16(input[i]);
    }
    return direct;
  }

  const sampleRateRatio = inputRate / TARGET_SAMPLE_RATE;
  const outputLength = Math.max(1, Math.round(input.length / sampleRateRatio));
  const output = new Int16Array(outputLength);
  let inputOffset = 0;

  for (let outputOffset = 0; outputOffset < outputLength; outputOffset += 1) {
    const nextInputOffset = Math.round((outputOffset + 1) * sampleRateRatio);
    let sum = 0;
    let count = 0;
    for (let i = inputOffset; i < nextInputOffset && i < input.length; i += 1) {
      sum += input[i];
      count += 1;
    }
    output[outputOffset] = clampToInt16(count > 0 ? sum / count : 0);
    inputOffset = nextInputOffset;
  }

  return output;
}

function encodePcmBase64(samples: Int16Array): string {
  const bytes = new Uint8Array(samples.length * 2);
  for (let i = 0; i < samples.length; i += 1) {
    const sample = samples[i];
    bytes[i * 2] = sample & 0xff;
    bytes[i * 2 + 1] = (sample >> 8) & 0xff;
  }

  let binary = "";
  for (let i = 0; i < bytes.length; i += BYTE_CHUNK_SIZE) {
    binary += String.fromCharCode(...bytes.subarray(i, i + BYTE_CHUNK_SIZE));
  }
  return btoa(binary);
}

export function useDashScopeAsr({
  workspaceId,
  enabled,
  onTranscriptReady,
}: UseDashScopeAsrOptions): UseDashScopeAsrResult {
  const [isRecording, setIsRecording] = useState(false);
  const [transcript, setTranscript] = useState("");
  const [error, setError] = useState<string | null>(null);

  const streamRef = useRef<MediaStream | null>(null);
  const audioContextRef = useRef<AudioContext | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const processorRef = useRef<ScriptProcessorNode | null>(null);
  const sinkGainRef = useRef<GainNode | null>(null);
  const sampleBufferRef = useRef<number[]>([]);
  const sendQueueRef = useRef(Promise.resolve());
  const finishingRef = useRef(false);
  const stoppedRef = useRef(false);
  const onTranscriptReadyRef = useRef(onTranscriptReady);
  onTranscriptReadyRef.current = onTranscriptReady;
  const unmountedRef = useRef(false);
  useEffect(() => {
    return () => {
      unmountedRef.current = true;
    };
  }, []);

  const cleanupMedia = useCallback(async () => {
    const processor = processorRef.current;
    const source = sourceRef.current;
    const sinkGain = sinkGainRef.current;
    const audioContext = audioContextRef.current;
    const stream = streamRef.current;

    processorRef.current = null;
    sourceRef.current = null;
    sinkGainRef.current = null;
    audioContextRef.current = null;
    streamRef.current = null;
    sampleBufferRef.current = [];

    if (processor) {
      processor.onaudioprocess = null;
      processor.disconnect();
    }
    source?.disconnect();
    sinkGain?.disconnect();
    stream?.getTracks().forEach((track) => track.stop());

    if (audioContext) {
      await audioContext.close().catch(() => undefined);
    }
  }, []);

  const queueAudioChunk = useCallback(
    (audioBase64: string) => {
      sendQueueRef.current = sendQueueRef.current
        .then(() =>
          invoke("dispatcher_append_voice_audio", {
            workspaceId,
            audioBase64,
          }).then(() => undefined),
        )
        .catch((reason) => {
          console.error("dispatcher_append_voice_audio 失败:", reason);
          setError(String(reason));
          setIsRecording(false);
          void cleanupMedia();
          void invoke("dispatcher_cancel_voice_input", { workspaceId }).catch(() => undefined);
        });
    },
    [cleanupMedia, workspaceId],
  );

  const flushBufferedSamples = useCallback(() => {
    while (sampleBufferRef.current.length >= CHUNK_SAMPLES) {
      const chunk = sampleBufferRef.current.splice(0, CHUNK_SAMPLES);
      queueAudioChunk(encodePcmBase64(Int16Array.from(chunk)));
    }
  }, [queueAudioChunk]);

  const finalizeRecording = useCallback(
    async (mode: "finish" | "cancel") => {
      if (stoppedRef.current) {
        return;
      }
      stoppedRef.current = true;
      setIsRecording(false);
      const remainingSamples =
        mode === "finish" ? sampleBufferRef.current.splice(0, sampleBufferRef.current.length) : [];
      await cleanupMedia();

      if (mode === "finish") {
        if (remainingSamples.length > 0) {
          queueAudioChunk(encodePcmBase64(Int16Array.from(remainingSamples)));
        }
        await sendQueueRef.current.catch(() => undefined);
        await invoke("dispatcher_finish_voice_input", { workspaceId }).catch((reason) => {
          console.error("dispatcher_finish_voice_input 失败:", reason);
          setError(String(reason));
        });
      } else {
        await invoke("dispatcher_cancel_voice_input", { workspaceId }).catch(() => undefined);
      }
    },
    [cleanupMedia, queueAudioChunk, workspaceId],
  );

  const startRecording = useCallback(async () => {
    if (!enabled || isRecording) {
      return;
    }
    if (!navigator.mediaDevices?.getUserMedia) {
      setError("当前环境不支持麦克风录音。");
      return;
    }

    setError(null);
    setTranscript("");
    finishingRef.current = false;
    stoppedRef.current = false;
    sampleBufferRef.current = [];
    sendQueueRef.current = Promise.resolve();

    try {
      await invoke("dispatcher_start_voice_input", { workspaceId });

      const stream = await navigator.mediaDevices.getUserMedia({
        audio: {
          channelCount: 1,
          sampleRate: TARGET_SAMPLE_RATE,
          echoCancellation: true,
          noiseSuppression: true,
          autoGainControl: true,
        },
      });

      const audioContext = new AudioContext();
      const source = audioContext.createMediaStreamSource(stream);
      const processor = audioContext.createScriptProcessor(4096, 1, 1);
      const sinkGain = audioContext.createGain();
      sinkGain.gain.value = 0;

      processor.onaudioprocess = (event) => {
        if (stoppedRef.current) {
          return;
        }
        const input = event.inputBuffer.getChannelData(0);
        const downsampled = downsampleTo16k(input, audioContext.sampleRate);
        for (const sample of downsampled) {
          sampleBufferRef.current.push(sample);
        }
        flushBufferedSamples();

        const output = event.outputBuffer.getChannelData(0);
        output.fill(0);
      };

      source.connect(processor);
      processor.connect(sinkGain);
      sinkGain.connect(audioContext.destination);

      streamRef.current = stream;
      audioContextRef.current = audioContext;
      sourceRef.current = source;
      processorRef.current = processor;
      sinkGainRef.current = sinkGain;
      setIsRecording(true);
    } catch (reason) {
      console.error("启动语音识别失败:", reason);
      setError(String(reason));
      setIsRecording(false);
      await cleanupMedia();
      await invoke("dispatcher_cancel_voice_input", { workspaceId }).catch(() => undefined);
    }
  }, [cleanupMedia, enabled, flushBufferedSamples, isRecording, workspaceId]);

  const stopRecording = useCallback(async () => {
    await finalizeRecording("finish");
  }, [finalizeRecording]);

  const toggleRecording = useCallback(async () => {
    if (isRecording) {
      await stopRecording();
      return;
    }
    await startRecording();
  }, [isRecording, startRecording, stopRecording]);

  useEffect(() => {
    const unlistenPromise = listen<DispatcherAsrEvent>("dispatcher-asr", (event) => {
      if (event.payload.workspaceId !== workspaceId) {
        return;
      }

      switch (event.payload.kind) {
        case "started":
          setError(null);
          break;
        case "partial":
          setTranscript(event.payload.text ?? "");
          break;
        case "final": {
          const text = (event.payload.text ?? "").trim();
          if (!text) {
            break;
          }
          setTranscript(text);
          if (finishingRef.current) {
            break;
          }
          finishingRef.current = true;
          void (async () => {
            await finalizeRecording("finish");
            if (unmountedRef.current) return;
            await onTranscriptReadyRef.current(text);
          })();
          break;
        }
        case "finished":
          setIsRecording(false);
          break;
        case "cancelled":
          setIsRecording(false);
          setTranscript("");
          break;
        case "error":
          setError(event.payload.message ?? "语音识别失败。");
          setIsRecording(false);
          void cleanupMedia();
          break;
      }
    });

    return () => {
      unlistenPromise.then((unlisten) => unlisten()).catch(() => {});
    };
  }, [cleanupMedia, finalizeRecording, workspaceId]);

  useEffect(() => {
    return () => {
      void cleanupMedia();
      void invoke("dispatcher_cancel_voice_input", { workspaceId }).catch(() => undefined);
    };
  }, [cleanupMedia, workspaceId]);

  return {
    isRecording,
    transcript,
    error,
    startRecording,
    stopRecording,
    toggleRecording,
    clearError: () => setError(null),
  };
}
