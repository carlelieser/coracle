#!/usr/bin/env node
/**
 * Differential trace comparator.
 *
 *   diff.mjs <expected.cdt> <actual.cdt> [--fp-policy=...] [--history=N]
 *            [--allow-feature-mismatch]
 *
 * Exit 0 on match, 1 on divergence, 2 on usage or format error.
 */
import { compareTraces } from "./compare.mjs";
import { POLICIES, makeComparator } from "./fp_policy.mjs";
import { StreamFlags, hex } from "./format.mjs";
import { formatDivergence, formatMatch } from "./report.mjs";
import { loadTrace } from "./reader.mjs";

const EXIT_MATCH = 0;
const EXIT_DIVERGED = 1;
const EXIT_ERROR = 2;

function parseArgs(argv) {
  const positional = [];
  const options = { fpPolicy: null, history: 8, allowFeatureMismatch: false };
  for (const arg of argv) {
    if (arg.startsWith("--fp-policy=")) {
      options.fpPolicy = arg.slice("--fp-policy=".length);
    } else if (arg.startsWith("--history=")) {
      options.history = Number(arg.slice("--history=".length));
    } else if (arg === "--allow-feature-mismatch") {
      options.allowFeatureMismatch = true;
    } else if (arg.startsWith("--")) {
      throw new Error(`unknown option '${arg}'`);
    } else {
      positional.push(arg);
    }
  }
  if (positional.length !== 2) {
    throw new Error("expected exactly two trace paths");
  }
  return { paths: positional, options };
}

/** Both streams precise => bitwise is the strict default; otherwise relax. */
function choosePolicy(requested, leftTrace, rightTrace) {
  const isPrecise = (trace) => (trace.header.flags & StreamFlags.PRECISE_FP) !== 0n;
  const bothPrecise = isPrecise(leftTrace) && isPrecise(rightTrace);

  if (requested === null) {
    return { policy: bothPrecise ? "bitwise" : "nan-insensitive", warning: null };
  }
  if (!POLICIES.includes(requested)) {
    throw new Error(
      `unknown FP policy '${requested}'; expected one of ${POLICIES.join(", ")}`,
    );
  }
  const warning =
    bothPrecise && requested !== "bitwise"
      ? `warning: both traces are PRECISE_FP but policy '${requested}' was requested;` +
        " this weakens a gate that could be strict"
      : null;
  return { policy: requested, warning };
}

function assertComparable(leftTrace, rightTrace, allowMismatch) {
  const left = leftTrace.header.cpuFeatureId;
  const right = rightTrace.header.cpuFeatureId;
  if (left === right || allowMismatch) return;
  throw new Error(
    `feature-mask mismatch: ${leftTrace.path} advertises ${hex(left)} but ` +
      `${rightTrace.path} advertises ${hex(right)}.\n` +
      "  Comparing traces from differently-configured CPUs produces noise, not signal.\n" +
      "  Fix the -cpu flags, or pass --allow-feature-mismatch if this is deliberate.",
  );
}

function main() {
  let parsed;
  try {
    parsed = parseArgs(process.argv.slice(2));
  } catch (error) {
    console.error(`error: ${error.message}`);
    console.error(
      "usage: diff.mjs <expected.cdt> <actual.cdt> [--fp-policy=bitwise|nan-insensitive|ignore-fpsr] [--history=N] [--allow-feature-mismatch]",
    );
    return EXIT_ERROR;
  }

  const [leftPath, rightPath] = parsed.paths;
  let leftTrace, rightTrace, policy, warning;
  try {
    leftTrace = loadTrace(leftPath);
    rightTrace = loadTrace(rightPath);
    assertComparable(leftTrace, rightTrace, parsed.options.allowFeatureMismatch);
    ({ policy, warning } = choosePolicy(
      parsed.options.fpPolicy,
      leftTrace,
      rightTrace,
    ));
  } catch (error) {
    console.error(`error: ${error.message}`);
    return EXIT_ERROR;
  }

  if (warning) console.error(warning);

  const result = compareTraces(leftTrace, rightTrace, {
    areEqual: makeComparator(policy),
    historyDepth: parsed.options.history,
  });

  if (result.ok) {
    console.log(formatMatch(result, leftTrace, rightTrace));
    console.log(`  fp policy: ${policy}`);
    return EXIT_MATCH;
  }
  console.log(formatDivergence(result, leftTrace, rightTrace));
  console.log(`\n  fp policy: ${policy}`);
  return EXIT_DIVERGED;
}

process.exit(main());
