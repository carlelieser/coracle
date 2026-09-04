#!/usr/bin/env node
/**
 * Asserts that a trace's exception records carry full architectural state,
 * whatever per-block scope the producer used. The M2 gate compares state at
 * every exception entry, so this must hold even at the fastest scope.
 */
import { RecordType, SYSREG_NAMES } from "./format.mjs";
import { loadTrace } from "./reader.mjs";

const path = process.argv[2];
if (!path) {
  console.error("usage: assert_exception_state.mjs <trace.cdt>");
  process.exit(2);
}

const trace = loadTrace(path);
const exceptions = trace.records.filter((r) => r.type === RecordType.EXCEPTION);
if (exceptions.length === 0) {
  console.error(`error: ${path} contains no exception records to check`);
  process.exit(1);
}

/* SPSR_EL1 and ELR_EL1 are written by the hardware on every synchronous
 * exception entry, so a full-state record must show them non-zero. */
const REQUIRED = ["SPSR_EL1", "ELR_EL1", "VBAR_EL1", "ESR_EL1"];
let failures = 0;

for (const [index, record] of exceptions.entries()) {
  for (const name of REQUIRED) {
    const value = record.sysreg[SYSREG_NAMES.indexOf(name)];
    if (value === 0n) {
      console.error(`exception ${index}: ${name} is zero; state is not full`);
      failures++;
    }
  }
  if (record.x.length !== 32 || record.v.length !== 64) {
    console.error(`exception ${index}: truncated register arrays`);
    failures++;
  }
}

if (failures > 0) process.exit(1);
console.log(`${exceptions.length} exception record(s) carry full state`);
