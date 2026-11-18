// PCM16 mono as FLAC in fragmented MP4, for a MediaSource on an audio element (docs/voice.md, the stream way out).
// The FLAC frames are verbatim: the samples as they are, so no encoder is needed and the browser only demuxes.
// No DOM here: voice.js feeds it, and node can check its output with ffmpeg.

/** Samples per FLAC frame and per MP4 sample: 20 ms at 16 kHz. */
export const BLOCK = 320;

const RATE_CODE = { 8000: 0x4, 16000: 0x5, 22050: 0x6, 24000: 0x7, 32000: 0x8, 44100: 0x9, 48000: 0xa };

function crc8(b) {
  let c = 0;
  for (const x of b) {
    c ^= x;
    for (let i = 0; i < 8; i++) c = c & 0x80 ? ((c << 1) ^ 0x07) & 0xff : (c << 1) & 0xff;
  }
  return c;
}

function crc16(b) {
  let c = 0;
  for (const x of b) {
    c ^= x << 8;
    for (let i = 0; i < 8; i++) c = c & 0x8000 ? ((c << 1) ^ 0x8005) & 0xffff : (c << 1) & 0xffff;
  }
  return c;
}

// The frame number in FLAC's UTF-8-like coding.
function utf8(n) {
  if (n < 0x80) return [n];
  const out = [];
  let len = n < 0x800 ? 2 : n < 0x10000 ? 3 : n < 0x200000 ? 4 : n < 0x4000000 ? 5 : 6;
  for (let i = len - 1; i > 0; i--) {
    out.unshift(0x80 | (n & 0x3f));
    n = Math.floor(n / 64);
  }
  out.unshift(((0xff00 >> len) & 0xff) | n);
  return out;
}

/** One verbatim FLAC frame of BLOCK samples, mono, 16 bit. */
export function flacFrame(num, pcm, rate) {
  const head = [0xff, 0xf8, 0x70 | RATE_CODE[rate], 0x08, ...utf8(num), (BLOCK - 1) >> 8, (BLOCK - 1) & 0xff];
  head.push(crc8(head));
  const out = new Uint8Array(head.length + 1 + BLOCK * 2 + 2);
  out.set(head);
  let at = head.length;
  // Subframe header: verbatim, no wasted bits.
  out[at++] = 0x02;
  for (let i = 0; i < BLOCK; i++) {
    const v = pcm[i] & 0xffff;
    out[at++] = v >> 8;
    out[at++] = v & 0xff;
  }
  const c = crc16(out.subarray(0, at));
  out[at++] = c >> 8;
  out[at] = c & 0xff;
  return out;
}

// An MP4 box: size, type, then the parts (byte arrays) in order.
function box(type, ...parts) {
  const size = 8 + parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(size);
  const v = new DataView(out.buffer);
  v.setUint32(0, size);
  for (let i = 0; i < 4; i++) out[4 + i] = type.charCodeAt(i);
  let at = 8;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

// Big-endian bytes from [value, byte count] pairs.
function be(...fields) {
  const out = [];
  for (const [v, n] of fields) for (let i = n - 1; i >= 0; i--) out.push(Math.floor(v / 2 ** (8 * i)) & 0xff);
  return new Uint8Array(out);
}

const full = (type, version, flags, ...parts) => box(type, be([version, 1], [flags, 3]), ...parts);
const ID = 1;

/** The init segment: one FLAC audio track at `rate`, timescale = rate. */
export function init(rate) {
  // STREAMINFO: block sizes, no frame sizes, rate (20 bits), 1 channel, 16 bits, unknown length, no MD5.
  const info = new Uint8Array(34);
  const iv = new DataView(info.buffer);
  iv.setUint16(0, BLOCK);
  iv.setUint16(2, BLOCK);
  iv.setUint32(10, (rate << 12) | (0 << 9) | (15 << 4));
  const dfla = full('dfLa', 0, 0, be([0x80, 1], [34, 3]), info);
  const entry = box('fLaC', be([0, 6], [1, 2], [0, 8], [1, 2], [16, 2], [0, 2], [0, 2], [rate * 65536, 4]), dfla);
  const matrix = be([0x10000, 4], [0, 4], [0, 4], [0, 4], [0x10000, 4], [0, 4], [0, 4], [0, 4], [0x40000000, 4]);
  const empty = (t) => full(t, 0, 0, be([0, 4]));
  return new Uint8Array([
    ...box('ftyp', new TextEncoder().encode('iso6'), be([0, 4]), new TextEncoder().encode('iso6mp41')),
    ...box('moov',
      full('mvhd', 0, 0, be([0, 4], [0, 4], [rate, 4], [0, 4], [0x10000, 4], [0x100, 2], [0, 10]), matrix, be([0, 24], [ID + 1, 4])),
      box('trak',
        full('tkhd', 0, 3, be([0, 4], [0, 4], [ID, 4], [0, 4], [0, 4], [0, 8], [0, 2], [0, 2], [0x100, 2], [0, 2]), matrix, be([0, 4], [0, 4])),
        box('mdia',
          full('mdhd', 0, 0, be([0, 4], [0, 4], [rate, 4], [0, 4], [0x55c4, 2], [0, 2])),
          full('hdlr', 0, 0, be([0, 4]), new TextEncoder().encode('soun'), be([0, 12]), new TextEncoder().encode('sound\0')),
          box('minf',
            full('smhd', 0, 0, be([0, 2], [0, 2])),
            box('dinf', full('dref', 0, 0, be([1, 4]), full('url ', 0, 1))),
            box('stbl', full('stsd', 0, 0, be([1, 4]), entry), empty('stts'), empty('stsc'), full('stsz', 0, 0, be([0, 4], [0, 4])), empty('stco'))))),
      box('mvex', full('trex', 0, 0, be([ID, 4], [1, 4], [BLOCK, 4], [0, 4], [0, 4])))),
  ]);
}

/**
 * A media segment: `pcm` (Int16Array, a whole number of blocks) starting at sample `at` of the track.
 * `seq` numbers the segments from 1; frames are numbered from the track's start.
 */
export function segment(seq, at, pcm, rate) {
  const n = pcm.length / BLOCK;
  const frames = [];
  for (let i = 0; i < n; i++) frames.push(flacFrame(at / BLOCK + i, pcm.subarray(i * BLOCK, (i + 1) * BLOCK), rate));
  const sizes = frames.map((f) => f.length);
  const trunBody = (offset) => be([n, 4], [offset, 4], ...sizes.map((s) => [s, 4]));
  // trun flags: data offset, per-sample sizes (durations come from trex); tfhd: the base is the moof.
  const build = (offset) => box('moof',
    full('mfhd', 0, 0, be([seq, 4])),
    box('traf',
      full('tfhd', 0, 0x020000, be([ID, 4])),
      full('tfdt', 1, 0, be([at, 8])),
      full('trun', 0, 0x000201, trunBody(offset))));
  const moofSize = build(0).length;
  const mdat = box('mdat', ...frames);
  const out = new Uint8Array(moofSize + mdat.length);
  out.set(build(moofSize + 8));
  out.set(mdat, moofSize);
  return out;
}
