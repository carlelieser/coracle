#!/usr/bin/env node
/**
 * Injects a single-register fault into a CDT trace.
 *
 * This is the differ's own test instrument: a differ that has only ever seen
 * matching traces is untested. It rewrites one register value in place,
 * preserving every offset, so the output is a byte-identical trace apart from
 * the injected fault.
 *
 *   perturb.mjs <in.cdt> <out.cdt> --reg=x2 --at-step=3 [--xor=0x1]
 */
import { readFileSync, writeFileSync } from "node:fs";

import {
  FILE_HEADER_BYTES, NUM_SYSREG, NUM_VREG, RECORD_HEADER_BYTES,
  RecordType, RegId, SYSREG_NAMES, hex,
} from "./format.mjs";

const BLOCK_PREFIX_BYTES = 32;
const REG_DELTA_BYTES = 16;

function parseRegName(name) {
  const gpr = /^x(\d+)$/.exec(name);
  if (gpr) return Number(gpr[1]);
  const named = { sp: RegId.SP, pc: RegId.PC, pstate: RegId.PSTATE,
                  fpcr: RegId.FPCR, fpsr: RegId.FPSR };
  if (name in named) return named[name];
  const vector = /^v(\d+)\.(lo|hi)$/.exec(name);
  if (vector) {
    return RegId.V_BASE + Number(vector[1]) * 2 + (vector[2] === "hi" ? 1 : 0);
  }
  const sysIndex = SYSREG_NAMES.indexOf(name);
  if (sysIndex >= 0) return RegId.SYS_BASE + sysIndex;
  throw new Error(`unrecognised register name '${name}'`);
}

/** Byte offset of a register's value within an exception record, or null. */
function exceptionValueOffset(regId) {
  const base = 8 + 32; // header + icount/from_pc/to_pc/discon_type/pad
  if (regId < 31) return base + regId * 8;
  if (regId === RegId.SP) return base + 31 * 8;
  const afterX = base + 32 * 8;
  if (regId === RegId.PC) return afterX;
  if (regId === RegId.PSTATE) return afterX + 8;
  const afterSys = afterX + 16 + NUM_SYSREG * 8;
  if (regId >= RegId.SYS_BASE && regId < RegId.SYS_BASE + NUM_SYSREG) {
    return afterX + 16 + (regId - RegId.SYS_BASE) * 8;
  }
  if (regId === RegId.FPCR) return afterSys;
  if (regId === RegId.FPSR) return afterSys + 8;
  if (regId >= RegId.V_BASE && regId < RegId.V_BASE + NUM_VREG * 2) {
    return afterSys + 16 + (regId - RegId.V_BASE) * 8;
  }
  return null;
}

/** Byte offset of a matching delta's value inside a block record, or null. */
function blockValueOffset(view, offset, deltaCount, regId) {
  for (let i = 0; i < deltaCount; i++) {
    const deltaAt = offset + BLOCK_PREFIX_BYTES + i * REG_DELTA_BYTES;
    if (view.getUint16(deltaAt, true) === regId) return deltaAt + 8;
  }
  return null;
}

function perturb(bytes, regId, targetStep, xorMask) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let offset = FILE_HEADER_BYTES;
  let step = 0;

  while (offset + RECORD_HEADER_BYTES <= bytes.length) {
    const type = view.getUint8(offset);
    const flags = view.getUint8(offset + 1);
    const length = view.getUint16(offset + 2, true);
    if (length < RECORD_HEADER_BYTES) {
      throw new Error(`malformed record at byte ${offset}`);
    }
    const isSignificant = type === RecordType.BLOCK || type === RecordType.EXCEPTION;
    if (isSignificant && step === targetStep) {
      const valueAt = type === RecordType.BLOCK
        ? blockValueOffset(view, offset, flags, regId)
        : (() => {
            const rel = exceptionValueOffset(regId);
            return rel === null ? null : offset + rel;
          })();
      if (valueAt === null) {
        throw new Error(
          `step ${targetStep} does not carry register id ${regId}; ` +
          "pick a step whose delta set includes it (see `cdt.mjs dump`)",
        );
      }
      const before = view.getBigUint64(valueAt, true);
      const after = before ^ xorMask;
      view.setBigUint64(valueAt, after, true);
      return { step, before, after };
    }
    if (isSignificant) step++;
    offset += length;
  }
  throw new Error(`trace has fewer than ${targetStep + 1} block/exception records`);
}

function parseArgs(argv) {
  const positional = [];
  const options = { reg: null, step: null, xor: 1n };
  for (const arg of argv) {
    if (arg.startsWith("--reg=")) options.reg = arg.slice(6);
    else if (arg.startsWith("--at-step=")) options.step = Number(arg.slice(10));
    else if (arg.startsWith("--xor=")) options.xor = BigInt(arg.slice(6));
    else if (arg.startsWith("--")) throw new Error(`unknown option '${arg}'`);
    else positional.push(arg);
  }
  if (positional.length !== 2) throw new Error("expected an input and an output path");
  if (options.reg === null) throw new Error("--reg is required");
  if (options.step === null) throw new Error("--at-step is required");
  return { paths: positional, options };
}

function main() {
  let parsed;
  try {
    parsed = parseArgs(process.argv.slice(2));
  } catch (error) {
    console.error(`error: ${error.message}`);
    console.error("usage: perturb.mjs <in.cdt> <out.cdt> --reg=x2 --at-step=3 [--xor=0x1]");
    return 2;
  }
  const [inputPath, outputPath] = parsed.paths;
  try {
    const regId = parseRegName(parsed.options.reg);
    const bytes = readFileSync(inputPath);
    const result = perturb(bytes, regId, parsed.options.step, parsed.options.xor);
    writeFileSync(outputPath, bytes);
    console.log(`perturbed ${parsed.options.reg} at step ${result.step}: ` +
                `${hex(result.before)} -> ${hex(result.after)}`);
    console.log(`wrote ${outputPath}`);
    return 0;
  } catch (error) {
    console.error(`error: ${error.message}`);
    return 2;
  }
}

process.exit(main());
