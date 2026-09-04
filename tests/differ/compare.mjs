/**
 * Aligns two CDT traces and finds the first divergence.
 *
 * Alignment: both producers agree on `icount`, the count of retired
 * instructions, so records are walked in lockstep by sequence. A control-flow
 * divergence shows up as a PC or n_insns mismatch on the same step, which is
 * reported before any register comparison is attempted.
 */
import { NUM_SYSREG, NUM_VREG, RecordType, RegId, SYSREG_NAMES, isFpReg } from "./format.mjs";

const KIND = {
  PC: "pc",
  ICOUNT: "icount",
  INSN_COUNT: "n_insns",
  REGISTER: "register",
  RECORD_TYPE: "record_type",
  LENGTH: "trace_length",
  EXCEPTION_FIELD: "exception_field",
};

/** Replays block deltas into a full register map so state can be compared. */
class RegisterState {
  constructor() {
    this.values = new Map();
  }

  applyDeltas(deltas) {
    for (const delta of deltas) {
      this.values.set(delta.regId, delta.value);
    }
  }

  applyException(record) {
    for (let n = 0; n < 31; n++) this.values.set(n, record.x[n]);
    this.values.set(RegId.SP, record.x[31]);
    this.values.set(RegId.PC, record.pc);
    this.values.set(RegId.PSTATE, record.pstate);
    this.values.set(RegId.FPCR, record.fpcr);
    this.values.set(RegId.FPSR, record.fpsr);
    for (let n = 0; n < NUM_VREG * 2; n++) {
      this.values.set(RegId.V_BASE + n, record.v[n]);
    }
    for (let n = 0; n < NUM_SYSREG; n++) {
      this.values.set(RegId.SYS_BASE + n, record.sysreg[n]);
    }
  }

  get(regId) {
    return this.values.get(regId) ?? 0n;
  }

  regIds() {
    return this.values.keys();
  }
}

function significantRecords(trace) {
  return trace.records.filter(
    (record) => record.type === RecordType.BLOCK ||
                record.type === RecordType.EXCEPTION,
  );
}

function compareRegisters(left, right, areEqual, context) {
  const regIds = new Set([...left.regIds(), ...right.regIds()]);
  for (const regId of [...regIds].sort((a, b) => a - b)) {
    const expected = left.get(regId);
    const actual = right.get(regId);
    if (expected === actual) continue;
    if (isFpReg(regId) && areEqual(regId, expected, actual)) continue;
    return { kind: KIND.REGISTER, regId, expected, actual, ...context };
  }
  return null;
}

function compareExceptionFields(left, right, context) {
  const fields = [
    ["from_pc", left.fromPc, right.fromPc],
    ["to_pc", left.toPc, right.toPc],
    ["discon_type", BigInt(left.disconType), BigInt(right.disconType)],
  ];
  for (const [name, expected, actual] of fields) {
    if (expected !== actual) {
      return { kind: KIND.EXCEPTION_FIELD, field: name, expected, actual, ...context };
    }
  }
  return null;
}

function compareStep(leftRecord, rightRecord, index) {
  const context = { index, icount: leftRecord.icount };
  if (leftRecord.type !== rightRecord.type) {
    return { kind: KIND.RECORD_TYPE, expected: BigInt(leftRecord.type),
             actual: BigInt(rightRecord.type), ...context };
  }
  if (leftRecord.icount !== rightRecord.icount) {
    return { kind: KIND.ICOUNT, expected: leftRecord.icount,
             actual: rightRecord.icount, ...context };
  }
  if (leftRecord.type === RecordType.BLOCK) {
    if (leftRecord.pc !== rightRecord.pc) {
      return { kind: KIND.PC, expected: leftRecord.pc, actual: rightRecord.pc, ...context };
    }
    if (leftRecord.nInsns !== rightRecord.nInsns) {
      return { kind: KIND.INSN_COUNT, expected: BigInt(leftRecord.nInsns),
               actual: BigInt(rightRecord.nInsns), ...context };
    }
    return null;
  }
  return compareExceptionFields(leftRecord, rightRecord, context);
}

/**
 * Returns { ok: true } or { ok: false, divergence, history }.
 * `history` is the last `historyDepth` aligned steps before the divergence.
 */
export function compareTraces(leftTrace, rightTrace, options = {}) {
  const { areEqual, historyDepth = 8 } = options;
  const left = significantRecords(leftTrace);
  const right = significantRecords(rightTrace);
  const leftState = new RegisterState();
  const rightState = new RegisterState();
  const history = [];

  const steps = Math.min(left.length, right.length);
  for (let index = 0; index < steps; index++) {
    const leftRecord = left[index];
    const rightRecord = right[index];

    const structural = compareStep(leftRecord, rightRecord, index);
    if (structural) return { ok: false, divergence: structural, history };

    if (leftRecord.type === RecordType.BLOCK) {
      leftState.applyDeltas(leftRecord.deltas);
      rightState.applyDeltas(rightRecord.deltas);
    } else {
      leftState.applyException(leftRecord);
      rightState.applyException(rightRecord);
    }

    const context = { index, icount: leftRecord.icount,
                      pc: leftRecord.type === RecordType.BLOCK
                          ? leftRecord.pc : leftRecord.toPc };
    const mismatch = compareRegisters(leftState, rightState, areEqual, context);
    if (mismatch) return { ok: false, divergence: mismatch, history };

    history.push({ index, icount: leftRecord.icount, type: leftRecord.type,
                   pc: context.pc,
                   nInsns: leftRecord.nInsns ?? 0 });
    if (history.length > historyDepth) history.shift();
  }

  if (left.length !== right.length) {
    return {
      ok: false,
      history,
      divergence: { kind: KIND.LENGTH, index: steps,
                    expected: BigInt(left.length), actual: BigInt(right.length) },
    };
  }
  return { ok: true, steps, history };
}

export { KIND as DivergenceKind };
