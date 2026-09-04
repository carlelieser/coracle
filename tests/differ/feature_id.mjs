#!/usr/bin/env node
/**
 * Computes the CDT `cpu_feature_id` from a feature-mask string, and (with
 * --verify) checks that a trace's header carries the expected value.
 *
 * This exists because the hash is a cross-implementation constant: the QEMU
 * plugin computes it in C and the emulator will compute it in Rust. A silent
 * disagreement would make the differ refuse every comparison.
 */
const OFFSET_BASIS = 0xcbf29ce484222325n;
const PRIME = 0x100000001b3n;
const MASK = (1n << 64n) - 1n;

export function featureId(text) {
  let hash = OFFSET_BASIS;
  for (const byte of Buffer.from(text, "latin1")) {
    hash = ((hash ^ BigInt(byte)) * PRIME) & MASK;
  }
  return hash;
}

if (import.meta.url === `file://${process.argv[1]}`) {
  const [text, tracePath] = process.argv.slice(2);
  if (!text) {
    console.error("usage: feature_id.mjs <feature-mask-string> [trace.cdt]");
    process.exit(2);
  }
  const expected = featureId(text);
  if (!tracePath) {
    console.log(`0x${expected.toString(16).padStart(16, "0")}`);
    process.exit(0);
  }
  const { loadTrace } = await import("./reader.mjs");
  const actual = loadTrace(tracePath).header.cpuFeatureId;
  if (actual !== expected) {
    console.error(
      `feature id mismatch for ${JSON.stringify(text)}:\n` +
      `  computed here: 0x${expected.toString(16).padStart(16, "0")}\n` +
      `  in ${tracePath}: 0x${actual.toString(16).padStart(16, "0")}`,
    );
    process.exit(1);
  }
  console.log(`feature id 0x${actual.toString(16).padStart(16, "0")} agrees`);
}
