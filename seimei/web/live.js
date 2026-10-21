// The live screen: H.264 from /api/stream/screen, decoded with WebCodecs onto a canvas. The server sends a config,
// then access units (flags u8, pts u64 µs, Annex B bytes); a benchmark pauses it. Without WebCodecs H.264 the
// Screen tab keeps its once-a-second stills.

export function liveSupported() {
  return typeof VideoDecoder !== 'undefined' && typeof EncodedVideoChunk !== 'undefined';
}

export class LiveStream {
  // onState(text, kind): kind is 'live', 'paused', 'error' or 'unsupported'.
  constructor(canvas, onState) {
    this.canvas = canvas;
    this.onState = onState;
    this.ws = null;
    this.dec = null;
    this.want = false;
    this.output = '';
    this.retry = 0;
    this.timer = 0;
  }

  start(output = '') {
    if (this.want && this.output === output && this.ws) return;
    this.stop();
    this.want = true;
    this.output = output;
    this.open();
  }

  stop() {
    this.want = false;
    clearTimeout(this.timer);
    if (this.ws) this.ws.close();
    this.ws = null;
    this.closeDecoder();
  }

  open() {
    const url = new URL(`api/stream/screen${this.output ? `?output=${encodeURIComponent(this.output)}` : ''}`, location.href);
    url.protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    ws.onmessage = (e) => { if (this.ws === ws) this.message(e.data); };
    ws.onclose = () => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.closeDecoder();
      if (!this.want) return;
      // Back off: 1 s, 2 s, 4 s ... 15 s.
      const wait = Math.min(15000, 1000 * 2 ** this.retry++);
      this.onState('Reconnecting…', 'error');
      this.timer = setTimeout(() => this.want && this.open(), wait);
    };
  }

  async message(data) {
    if (typeof data === 'string') {
      const m = JSON.parse(data);
      if (m.type === 'config') await this.configure(m);
      else if (m.type === 'paused') { this.closeDecoder(); this.onState('Paused while a benchmark runs', 'paused'); }
      else if (m.type === 'error') this.onState(m.text || 'The screen capture stopped', 'error');
      return;
    }
    if (!this.dec || this.dec.state !== 'configured') return;
    const head = new DataView(data, 0, 9);
    const key = (head.getUint8(0) & 1) === 1;
    if (this.needKey && !key) return;
    // A decoder that falls behind skips to the next keyframe instead of drifting late.
    if (this.dec.decodeQueueSize > 8 && !key) {
      this.needKey = true;
      this.ws?.send('key');
      return;
    }
    this.needKey = false;
    this.dec.decode(new EncodedVideoChunk({ type: key ? 'key' : 'delta', timestamp: Number(head.getBigUint64(1)), data: new Uint8Array(data, 9) }));
  }

  async configure(m) {
    this.closeDecoder();
    const cfg = { codec: m.codec, codedWidth: m.width, codedHeight: m.height, optimizeForLatency: true };
    const ok = await VideoDecoder.isConfigSupported(cfg).then((r) => r.supported, () => false);
    if (!ok) {
      this.onState(`This browser cannot decode ${m.codec}`, 'unsupported');
      this.stop();
      return;
    }
    this.canvas.width = m.width;
    this.canvas.height = m.height;
    const ctx = this.canvas.getContext('2d');
    this.dec = new VideoDecoder({
      output: (frame) => {
        ctx.drawImage(frame, 0, 0, this.canvas.width, this.canvas.height);
        frame.close();
        if (this.retry || !this.shown) { this.retry = 0; this.shown = true; this.onState('', 'live'); }
      },
      // An error closes the decoder: start over at the next keyframe.
      error: () => { this.ws?.send('key'); if (this.want) this.configure(m); },
    });
    this.dec.configure(cfg);
    this.needKey = true;
    this.shown = false;
    // The GOP sent on joining went by while the decoder was being set up: start from a fresh keyframe.
    this.ws?.send('key');
  }

  closeDecoder() {
    if (this.dec && this.dec.state !== 'closed') this.dec.close();
    this.dec = null;
  }
}
