/**
 * Human-readable divergence reports. The point of this file is that a report
 * should be actionable on its own: which instruction, which register, expected
 * vs actual, and the block history that led there.
 */
import { DivergenceKind } from "./compare.mjs";
import { RecordType, RegId, decodePstate, hex, regName } from "./format.mjs";

function describeValue(regId, value) {
  if (regId === RegId.PSTATE) return `${hex(value)}  [${decodePstate(value)}]`;
  return hex(value);
}

function headline(divergence) {
  switch (divergence.kind) {
    case DivergenceKind.REGISTER:
      return `register ${regName(divergence.regId)} differs`;
    case DivergenceKind.PC:
      return "control flow diverged (block PC differs)";
    case DivergenceKind.ICOUNT:
      return "instruction counts drifted apart";
    case DivergenceKind.INSN_COUNT:
      return "block length differs";
    case DivergenceKind.RECORD_TYPE:
      return "record types differ (one side took an exception, the other did not)";
    case DivergenceKind.EXCEPTION_FIELD:
      return `exception field ${divergence.field} differs`;
    case DivergenceKind.LENGTH:
      return "traces have different lengths but agree everywhere they overlap";
    default:
      return `divergence (${divergence.kind})`;
  }
}

function formatHistory(history, leftName) {
  if (history.length === 0) return ["  (no preceding blocks)"];
  return history.map((entry) => {
    const kind = entry.type === RecordType.EXCEPTION ? "EXCEPT" : "BLOCK ";
    return `  ${kind} step=${String(entry.index).padStart(6)}` +
           ` icount=${String(entry.icount).padStart(10)}` +
           ` pc=${hex(entry.pc)} n=${entry.nInsns}`;
  });
}

export function formatDivergence(result, leftTrace, rightTrace) {
  const { divergence, history } = result;
  const lines = [];
  const leftName = leftTrace.header.producerName || leftTrace.path;
  const rightName = rightTrace.header.producerName || rightTrace.path;

  lines.push("DIVERGENCE");
  lines.push(`  ${headline(divergence)}`);
  lines.push("");
  lines.push(`  step        ${divergence.index}`);
  if (divergence.icount !== undefined) {
    lines.push(`  icount      ${divergence.icount}`);
  }
  if (divergence.pc !== undefined) {
    lines.push(`  block pc    ${hex(divergence.pc)}`);
  }
  lines.push("");

  const regId = divergence.regId;
  const label = regId !== undefined ? regName(regId)
              : divergence.field ?? divergence.kind;
  lines.push(`  ${label}`);
  lines.push(`    expected (${leftName})  ${
    regId !== undefined ? describeValue(regId, divergence.expected)
                        : hex(divergence.expected)}`);
  lines.push(`    actual   (${rightName})  ${
    regId !== undefined ? describeValue(regId, divergence.actual)
                        : hex(divergence.actual)}`);
  if (regId !== undefined) {
    const delta = divergence.expected ^ divergence.actual;
    lines.push(`    xor                       ${hex(delta)}`);
  }
  lines.push("");
  lines.push("  preceding blocks (oldest first):");
  lines.push(...formatHistory(history, leftName));
  return lines.join("\n");
}

export function formatMatch(result, leftTrace, rightTrace) {
  return [
    "MATCH",
    `  ${result.steps} aligned records compared, no divergence`,
    `  expected: ${leftTrace.header.producerName || leftTrace.path}`,
    `  actual:   ${rightTrace.header.producerName || rightTrace.path}`,
  ].join("\n");
}
