#!/usr/bin/env node
// Repair dsh session logs corrupted by a harness writer bug (rc.6): a session
// log whose first Zstandard frame contains the whole log instead of one header
// line, and/or whose event seq stream restarts mid-log. Both break
// `session.list` (HTTP 500 -> empty workspace sidebar) until fixed.
//
// Usage: node scripts/repair-session-logs.mjs [--dry-run]
//
// - Scans ~/.dsh/sessions for `session.jsonl.zstd` files.
// - Re-encodes "whole log in first frame" files as header frame + events frames.
// - Renumbers seqs after writer-bug restarts so the stream is contiguous.
// - Backs up each touched file as `<file>.repair-bak`.
import { readFileSync, writeFileSync, renameSync, copyFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";
import { homedir } from "node:os";
import { promisify } from "node:util";
import { createRequire } from "node:module";
import { constants, zstdCompress, zstdDecompressSync } from "node:zlib";

const require = createRequire(import.meta.url);
const DRY = process.argv.includes("--dry-run");
const compress = promisify(zstdCompress);
const O = { params: { [constants.ZSTD_c_checksumFlag]: 1 } };

// decodeStorageRecord lives in @deepseek-ai/dsh-session. Resolve it from the
// dsh install (npx cache) or any node_modules that has it.
async function loadDecoder() {
  const candidates = [
    "/Users/hezhaozhao/.npm/_npx/1e7f6d9597241db0/node_modules/@deepseek-ai/dsh-session/lib/index.js",
  ];
  for (const p of candidates) {
    try { return (await import(p)).decodeStorageRecord; } catch {}
  }
  try { return require("@deepseek-ai/dsh-session").decodeStorageRecord; } catch {}
  throw new Error("cannot resolve @deepseek-ai/dsh-session; adjust loadDecoder()");
}
const decodeStorageRecord = await loadDecoder();

function collect(dir) {
  const out = [];
  for (const e of readdirSync(dir)) {
    const p = join(dir, e);
    if (statSync(p).isDirectory()) out.push(...collect(p));
    else if (e === "session.jsonl.zstd") out.push(p);
  }
  return out;
}

function scanFrames(buffer) {
  const ZSTD_MAGIC = 4247762216;
  const frames = [];
  let offset = 0;
  while (offset < buffer.length) {
    const start = offset;
    if (buffer.length - offset < 4) break;
    if (buffer.readUInt32LE(offset) !== ZSTD_MAGIC) throw new Error("invalid frame magic");
    offset += 4;
    const d = buffer.readUInt8(offset); offset += 1;
    const csf = d >>> 6, ss = (d & 32) !== 0, ck = (d & 4) !== 0, df = d & 3;
    offset += (ss ? 0 : 1) + (df === 3 ? 4 : df) + (csf === 0 ? (ss ? 1 : 0) : 1 << csf);
    for (;;) {
      const bh = buffer.readUIntLE(offset, 3); offset += 3;
      const last = (bh & 1) !== 0, t = (bh >>> 1) & 3, s = bh >>> 3;
      if (t === 3) throw new Error("reserved block type");
      offset += t === 1 ? 1 : s;
      if (last) break;
    }
    if (ck) offset += 4;
    frames.push(buffer.subarray(start, offset));
  }
  return frames;
}

function fullPlaintext(buf) {
  return scanFrames(buf).map((f) => zstdDecompressSync(f).toString()).join("");
}

function validate(lines) {
  let events = 0;
  for (let i = 1; i < lines.length; i++) {
    const o = JSON.parse(lines[i]);
    let decoded;
    try { decoded = decodeStorageRecord(o); } catch { return `line ${i}: decode threw`; }
    for (const ev of decoded) {
      if (ev.seq !== events) return `line ${i}: seq gap expected ${events} got ${ev.seq} type=${ev.type}`;
      events++;
    }
  }
  return events;
}

function reencode(header, lines) {
  const newPlain = header + lines.join("\n") + "\n";
  return newPlain;
}

const root = join(homedir(), ".dsh", "sessions");
const files = collect(root);
console.log(`scanning ${files.length} logs under ${root}${DRY ? " (dry-run)" : ""}`);
let fixed = 0;
for (const f of files) {
  let buf;
  try { buf = readFileSync(f); } catch { continue; }
  const frames = scanFrames(buf);
  if (frames.length === 0) { console.log("EMPTY/UNREADABLE:", f); continue; }
  // 1) framing check: first frame must be exactly one header line
  let first;
  try { first = zstdDecompressSync(frames[0]).toString(); } catch { console.log("FIRST FRAME UNDECODABLE:", f); continue; }
  const framingBad = first.length === 0 || first.indexOf("\n") !== first.length - 1 || !first.includes('"type":"session"');

  const plain = fullPlaintext(buf);
  const rawLines = plain.split("\n");
  const header = rawLines[0] + "\n";
  const lines = rawLines.slice(1).filter((l) => l.trim() !== "");

  // 2) seq continuity check
  const before = validate([header.trim(), ...lines]);
  const seqBad = typeof before !== "number";

  if (!framingBad && !seqBad) continue;

  console.log((framingBad ? "FRAMING" : "") + (framingBad && seqBad ? " + " : "") + (seqBad ? "SEQ" : "") + ": " + f.replace(root + "/", ""));
  console.log("   before: " + (seqBad ? before : "ok") + ", events: " + (typeof before === "number" ? before : "?"));

  // repair framing: header frame + one events frame
  const headerFrame = await compress(Buffer.from(header), O);
  const eventsFrame = await compress(Buffer.from(plain.slice(header.length)), O);

  // repair seq: renumber restarts (capture decoded seqs before mutating)
  let running = 0, offset = 0, restarts = 0;
  const out = [];
  for (const line of lines) {
    const o = JSON.parse(line);
    const decoded = decodeStorageRecord(o);
    const seqs = [];
    for (const ev of decoded) if (typeof ev.seq === "number") seqs.push(ev.seq);
    // A restart replays earlier seq values (seqs[0] < running). Missing-event
    // gaps (seqs[0] > running) are not renumberable; validation below skips.
    if (seqs[0] !== running && seqs[0] < running) { offset += running - seqs[0]; restarts++; }
    if (offset > 0) {
      if (o.seq !== undefined) o.seq += offset;
      if (o.seq0 !== undefined) o.seq0 += offset;
    }
    running = seqs.length ? seqs[seqs.length - 1] + 1 : running;
    out.push(JSON.stringify(o));
  }
  const after = validate([header.trim(), ...out]);
  if (typeof after !== "number") { console.log("   POST-REPAIR STILL INVALID:", after, "-> skipping"); continue; }

  if (!DRY) {
    const newPlain = header + out.join("\n") + "\n";
    const repaired = Buffer.concat([
      await compress(Buffer.from(header), O),
      await compress(Buffer.from(newPlain.slice(header.length)), O),
    ]);
    copyFileSync(f, f + ".repair-bak");
    writeFileSync(f + ".new", repaired);
    renameSync(f + ".new", f);
  }
  console.log(`   FIXED: ${restarts} seq restart(s), events now ${after}, bytes ${buf.length} -> ${DRY ? "(dry)" : "written"}`);
  fixed++;
}
console.log(`done, ${fixed} file(s) ${DRY ? "would be" : ""} repaired.`);
