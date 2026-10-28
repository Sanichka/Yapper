export interface PlaybackHandle {
  durationMs: number;
  stop(): void;
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

export class Player {
  private context: AudioContext | null = null;
  private gain: GainNode | null = null;
  private source: AudioBufferSourceNode | null = null;
  private volume = 1.0;

  get suspended(): boolean {
    return this.context?.state === "suspended";
  }

  getVolume(): number {
    return this.volume;
  }

  setVolume(value: number): void {
    const clamped = Math.min(1, Math.max(0, Number.isFinite(value) ? value : 1));
    this.volume = clamped;
    if (this.gain && this.context) {
      this.gain.gain.setValueAtTime(clamped, this.context.currentTime);
      this.gain.gain.value = clamped;
    } else if (this.gain) {
      this.gain.gain.value = clamped;
    }
  }

  private ensure(): AudioContext {
    if (!this.context) {
      this.context = new AudioContext({ latencyHint: "interactive" });
      this.gain = this.context.createGain();
      this.gain.gain.value = this.volume;
      this.gain.connect(this.context.destination);
    } else if (this.gain) {
      this.gain.gain.value = this.volume;
    }
    return this.context;
  }

  async unlock(): Promise<void> {
    const context = this.ensure();
    if (context.state !== "suspended") return;
    await Promise.race([
      context.resume().catch(() => undefined),
      new Promise((resolve) => window.setTimeout(resolve, 300)),
    ]);
  }

  async play(wavBase64: string, onEnded: () => void): Promise<PlaybackHandle> {
    const context = this.ensure();
    if (context.state === "suspended") {
      void context.resume().catch(() => undefined);
    }
    const bytes = base64ToBytes(wavBase64);
    const arrayBuffer = new ArrayBuffer(bytes.byteLength);
    new Uint8Array(arrayBuffer).set(bytes);
    const buffer = await context.decodeAudioData(arrayBuffer);
    const source = context.createBufferSource();
    source.buffer = buffer;
    if (this.gain) source.connect(this.gain);

    this.stop();
    this.source = source;
    source.onended = () => {
      if (this.source !== source) return;
      this.source = null;
      onEnded();
    };
    source.start();

    return {
      durationMs: buffer.duration * 1000,
      stop: () => {
        if (this.source === source) this.source = null;
        try {
          source.stop();
        } catch {
          // already stopped
        }
      },
    };
  }

  stop(): void {
    if (!this.source) return;
    const source = this.source;
    this.source = null;
    try {
      source.stop();
    } catch {
      // already stopped
    }
  }
}
