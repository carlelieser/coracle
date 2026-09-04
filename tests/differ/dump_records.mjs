/**
 * Dumps a CDT trace as JSON, so a producer's tests can assert against the
 * reader that will consume its stream at the gate rather than against their own
 * idea of the layout.
 *
 * Usage: node dump_records.mjs <trace.cdt>
 */
import { RecordType, StreamFlags, hex } from "./format.mjs";
import { loadTrace } from "./reader.mjs";

function summarise(trace) {
  const of = (type) => trace.records.filter((record) => record.type === type);
  const [end] = of(RecordType.END);

  return {
    header: {
      formatVersion: trace.header.formatVersion,
      producer: trace.header.producer,
      producerName: trace.header.producerName,
      cpuFeatureId: hex(trace.header.cpuFeatureId),
    },
    flags: {
      preciseFp: (trace.header.flags & StreamFlags.PRECISE_FP) !== 0n,
      hasVregs: (trace.header.flags & StreamFlags.HAS_VREGS) !== 0n,
      hasSysregs: (trace.header.flags & StreamFlags.HAS_SYSREGS) !== 0n,
      blockDeltas: (trace.header.flags & StreamFlags.BLOCK_DELTAS) !== 0n,
    },
    markers: of(RecordType.MARKER).map((record) => ({
      icount: Number(record.icount),
      kind: Number(record.kind),
      value: Number(record.value),
    })),
    blocks: of(RecordType.BLOCK).map((record) => ({
      pc: hex(record.pc),
      icount: Number(record.icount),
      nInsns: record.nInsns,
      deltas: record.deltas.map((delta) => ({
        regId: delta.regId,
        value: hex(delta.value),
      })),
    })),
    exceptions: of(RecordType.EXCEPTION).map((record) => ({
      icount: Number(record.icount),
      fromPc: hex(record.fromPc),
      toPc: hex(record.toPc),
      disconType: record.disconType,
      x: record.x.map(hex),
      pc: hex(record.pc),
      pstate: hex(record.pstate),
      sysreg: record.sysreg.map(hex),
      fpcr: hex(record.fpcr),
      fpsr: hex(record.fpsr),
      v: record.v.map(hex),
    })),
    end: end ? { icount: Number(end.icount), reason: Number(end.reason) } : null,
  };
}

const [path] = process.argv.slice(2);
if (!path) {
  console.error("usage: node dump_records.mjs <trace.cdt>");
  process.exit(2);
}

process.stdout.write(`${JSON.stringify(summarise(loadTrace(path)), null, 2)}\n`);
