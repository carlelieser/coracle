#!/usr/bin/env node
/**
 * CDT trace inspection CLI. Companion to `diff.mjs`; used for eyeballing a
 * trace and for scripted checks in the harness.
 */
import {
  DISCON_NAMES, PRODUCER, RecordType, StreamFlags,
  decodePstate, hex, regName,
} from "./format.mjs";
import { loadTrace } from "./reader.mjs";

function flagNames(flags) {
  return Object.entries(StreamFlags)
    .filter(([, bit]) => (flags & bit) !== 0n)
    .map(([name]) => name)
    .join(",") || "none";
}

function showHeader(trace) {
  const { header } = trace;
  console.log(`file:      ${trace.path}`);
  console.log(`producer:  ${PRODUCER[header.producer] ?? header.producer} (${header.producerName})`);
  console.log(`flags:     ${flagNames(header.flags)}`);
  console.log(`cpu id:    ${hex(header.cpuFeatureId)}`);
  console.log(`records:   ${trace.records.length}`);
}

function describe(record) {
  switch (record.type) {
    case RecordType.BLOCK: {
      const deltas = record.deltas
        .map((d) => `${regName(d.regId)}=${hex(d.value)}`)
        .join(" ");
      return `BLOCK   icount=${record.icount} pc=${hex(record.pc)} n=${record.nInsns} ${deltas}`;
    }
    case RecordType.EXCEPTION:
      return `EXCEPT  icount=${record.icount} ${DISCON_NAMES[record.disconType] ?? record.disconType}` +
             ` from=${hex(record.fromPc)} to=${hex(record.toPc)} pstate=[${decodePstate(record.pstate)}]`;
    case RecordType.MARKER:
      return `MARKER  icount=${record.icount} kind=${record.kind} value=${record.value}`;
    case RecordType.END:
      return `END     icount=${record.icount} reason=${record.reason}`;
    default:
      return `UNKNOWN type=${record.type}`;
  }
}

function commandDump(path, limit) {
  const trace = loadTrace(path);
  showHeader(trace);
  console.log("");
  const shown = limit ? trace.records.slice(0, limit) : trace.records;
  for (const record of shown) console.log(describe(record));
  if (shown.length < trace.records.length) {
    console.log(`... ${trace.records.length - shown.length} more records`);
  }
}

function commandStats(path) {
  const trace = loadTrace(path);
  const counts = new Map();
  let deltas = 0;
  let instructions = 0n;
  for (const record of trace.records) {
    counts.set(record.type, (counts.get(record.type) ?? 0) + 1);
    if (record.type === RecordType.BLOCK) {
      deltas += record.deltas.length;
      instructions += BigInt(record.nInsns);
    }
  }
  showHeader(trace);
  console.log("");
  for (const [type, count] of [...counts].sort((a, b) => a[0] - b[0])) {
    const name = Object.entries(RecordType).find(([, v]) => v === type)?.[0] ?? type;
    console.log(`${String(name).padEnd(10)} ${count}`);
  }
  const blocks = counts.get(RecordType.BLOCK) ?? 0;
  console.log(`\ninstructions ${instructions}`);
  console.log(`deltas       ${deltas} (${blocks ? (deltas / blocks).toFixed(2) : 0} per block)`);
}

function commandCountExceptions(path) {
  const trace = loadTrace(path);
  const count = trace.records.filter((r) => r.type === RecordType.EXCEPTION).length;
  console.log(count);
}

/** Exit 0 only if the trace is readable and ends with a REC_END. */
function commandIsComplete(path) {
  const trace = loadTrace(path);
  const last = trace.records.at(-1);
  if (last?.type !== RecordType.END) {
    process.exit(1);
  }
}

const [command, path, extra] = process.argv.slice(2);
if (!command || !path) {
  console.error("usage: cdt.mjs <dump|stats|count-exceptions|is-complete> <trace.cdt> [limit]");
  process.exit(2);
}

const commands = {
  dump: () => commandDump(path, extra ? Number(extra) : 0),
  stats: () => commandStats(path),
  "count-exceptions": () => commandCountExceptions(path),
  "is-complete": () => commandIsComplete(path),
};

const handler = commands[command];
if (!handler) {
  console.error(`unknown command '${command}'`);
  process.exit(2);
}
handler();
