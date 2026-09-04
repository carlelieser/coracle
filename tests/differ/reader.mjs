/**
 * Streaming reader for CDT v1 traces.
 */
import { readFileSync } from "node:fs";

import {
  FILE_HEADER_BYTES, FORMAT_VERSION, MAGIC, NUM_SYSREG, NUM_VREG,
  RECORD_HEADER_BYTES, REG_DELTA_BYTES, RecordType,
} from "./format.mjs";

export class TraceFormatError extends Error {
  constructor(path, detail) {
    super(`invalid CDT trace '${path}': ${detail}`);
    this.name = "TraceFormatError";
  }
}

function readHeader(view, path) {
  const magic = Buffer.from(view.buffer, view.byteOffset, 8).toString("latin1");
  if (magic !== MAGIC) {
    throw new TraceFormatError(path, `bad magic ${JSON.stringify(magic)}`);
  }
  const formatVersion = view.getUint32(8, true);
  if (formatVersion !== FORMAT_VERSION) {
    throw new TraceFormatError(path, `format version ${formatVersion}, expected ${FORMAT_VERSION}`);
  }
  const nameBytes = Buffer.from(view.buffer, view.byteOffset + 32, 32);
  const end = nameBytes.indexOf(0);
  return {
    formatVersion,
    producer: view.getUint32(12, true),
    flags: view.getBigUint64(16, true),
    cpuFeatureId: view.getBigUint64(24, true),
    producerName: nameBytes.toString("latin1", 0, end < 0 ? 32 : end),
  };
}

/* hdr(8) + pc(8) + icount(8) + n_insns(2) + pad(6) */
const BLOCK_PREFIX_BYTES = 32;

function readBlock(view, offset, count) {
  const deltas = [];
  let cursor = offset + BLOCK_PREFIX_BYTES;
  for (let i = 0; i < count; i++) {
    deltas.push({
      regId: view.getUint16(cursor, true),
      value: view.getBigUint64(cursor + 8, true),
    });
    cursor += REG_DELTA_BYTES;
  }
  return {
    type: RecordType.BLOCK,
    pc: view.getBigUint64(offset + 8, true),
    icount: view.getBigUint64(offset + 16, true),
    nInsns: view.getUint16(offset + 24, true),
    deltas,
  };
}

function readWords(view, offset, count) {
  const words = new Array(count);
  for (let i = 0; i < count; i++) {
    words[i] = view.getBigUint64(offset + i * 8, true);
  }
  return words;
}

function readException(view, offset) {
  let cursor = offset + 8;
  const icount = view.getBigUint64(cursor, true);
  const fromPc = view.getBigUint64(cursor + 8, true);
  const toPc = view.getBigUint64(cursor + 16, true);
  const disconType = view.getUint32(cursor + 24, true);
  cursor += 32;
  const x = readWords(view, cursor, 32); cursor += 32 * 8;
  const pc = view.getBigUint64(cursor, true); cursor += 8;
  const pstate = view.getBigUint64(cursor, true); cursor += 8;
  const sysreg = readWords(view, cursor, NUM_SYSREG); cursor += NUM_SYSREG * 8;
  const fpcr = view.getBigUint64(cursor, true); cursor += 8;
  const fpsr = view.getBigUint64(cursor, true); cursor += 8;
  const v = readWords(view, cursor, NUM_VREG * 2);
  return { type: RecordType.EXCEPTION, icount, fromPc, toPc, disconType,
           x, pc, pstate, sysreg, fpcr, fpsr, v };
}

function readRecord(view, offset, path) {
  const type = view.getUint8(offset);
  const flags = view.getUint8(offset + 1);
  const length = view.getUint16(offset + 2, true);
  if (length < RECORD_HEADER_BYTES || length % 8 !== 0) {
    throw new TraceFormatError(path, `record at ${offset} has length ${length}`);
  }
  switch (type) {
    case RecordType.BLOCK:
      return { length, record: readBlock(view, offset, flags) };
    case RecordType.EXCEPTION:
      return { length, record: readException(view, offset) };
    case RecordType.MARKER:
      return { length, record: { type, icount: view.getBigUint64(offset + 8, true),
                                 kind: view.getBigUint64(offset + 16, true),
                                 value: view.getBigUint64(offset + 24, true) } };
    case RecordType.END:
      return { length, record: { type, icount: view.getBigUint64(offset + 8, true),
                                 reason: view.getBigUint64(offset + 16, true) } };
    default:
      // Unknown type: `length` still lets us advance. Forward compatibility.
      return { length, record: { type, unknown: true } };
  }
}

export function loadTrace(path) {
  const bytes = readFileSync(path);
  if (bytes.length < FILE_HEADER_BYTES) {
    throw new TraceFormatError(path, `file is ${bytes.length} bytes, shorter than a header`);
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const header = readHeader(view, path);
  const records = [];
  let offset = FILE_HEADER_BYTES;
  while (offset + RECORD_HEADER_BYTES <= bytes.length) {
    const { length, record } = readRecord(view, offset, path);
    if (offset + length > bytes.length) {
      throw new TraceFormatError(path, `record at ${offset} runs past end of file`);
    }
    records.push(record);
    offset += length;
  }
  return { path, header, records };
}
