#!/usr/bin/env node
/**
 * Tests the FP comparison policies (TRACE_FORMAT.md §7). The differential FP
 * legs depend on these being right: too strict and every native-backend run is
 * noise, too loose and the gate stops catching real divergence.
 */
import { makeComparator } from "./fp_policy.mjs";
import { RegId } from "./format.mjs";

const V0 = RegId.V_BASE;
const QNAN_64 = 0x7ff8000000000000n;
const QNAN_64_OTHER_PAYLOAD = 0x7ff8000000abcdefn;
const SNAN_64 = 0x7ff4000000000000n;
const INF_64 = 0x7ff0000000000000n;
const ONE_64 = 0x3ff0000000000000n;

let failures = 0;

function expect(description, actual, wanted) {
  if (actual === wanted) {
    console.log(`    ok    ${description}`);
  } else {
    console.log(`    FAIL  ${description} (got ${actual}, wanted ${wanted})`);
    failures++;
  }
}

const bitwise = makeComparator("bitwise");
const nanInsensitive = makeComparator("nan-insensitive");
const ignoreFpsr = makeComparator("ignore-fpsr");

console.log("  bitwise:");
expect("equal values match", bitwise(V0, ONE_64, ONE_64), true);
expect(
  "differing NaN payloads do NOT match",
  bitwise(V0, QNAN_64, QNAN_64_OTHER_PAYLOAD),
  false,
);

console.log("  nan-insensitive:");
expect(
  "differing quiet-NaN payloads match",
  nanInsensitive(V0, QNAN_64, QNAN_64_OTHER_PAYLOAD),
  true,
);
expect(
  "quiet vs signalling NaN do NOT match",
  nanInsensitive(V0, QNAN_64, SNAN_64),
  false,
);
expect("NaN vs infinity do NOT match", nanInsensitive(V0, QNAN_64, INF_64), false);
expect(
  "distinct finite values do NOT match",
  nanInsensitive(V0, ONE_64, INF_64),
  false,
);
expect(
  "differing FPCR is still a divergence",
  nanInsensitive(RegId.FPCR, 0n, 1n),
  false,
);
expect(
  "differing FPSR is still a divergence",
  nanInsensitive(RegId.FPSR, 0n, 0x10n),
  false,
);

console.log("  ignore-fpsr:");
expect("FPSR cumulative bits are ignored", ignoreFpsr(RegId.FPSR, 0x00n, 0x1fn), true);
expect(
  "FPSR non-cumulative bits still compared",
  ignoreFpsr(RegId.FPSR, 0x00n, 0x8000000n),
  false,
);
expect(
  "NaN payloads still ignored under this policy",
  ignoreFpsr(V0, QNAN_64, QNAN_64_OTHER_PAYLOAD),
  true,
);

process.exit(failures === 0 ? 0 : 1);
