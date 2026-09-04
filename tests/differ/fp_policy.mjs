/**
 * FP comparison policies (TRACE_FORMAT.md §7).
 *
 * The plan runs differential FP legs in "precise mode" (softfloat everywhere),
 * where bitwise equality holds. The native-wasm FP backend is compared under a
 * NaN-payload-insensitive policy instead, because wasm gives no control over
 * NaN payload propagation.
 */
import { RegId } from "./format.mjs";

export const POLICIES = ["bitwise", "nan-insensitive", "ignore-fpsr"];

const EXPONENT_ALL_ONES_64 = 0x7ffn << 52n;
const MANTISSA_64 = (1n << 52n) - 1n;
const QUIET_BIT_64 = 1n << 51n;

const EXPONENT_ALL_ONES_32 = 0xffn << 23n;
const MANTISSA_32 = (1n << 23n) - 1n;
const QUIET_BIT_32 = 1n << 22n;

/** FPSR cumulative exception bits: IOC DZC OFC UFC IXC IDC. */
const FPSR_CUMULATIVE_MASK = 0x9fn;

function classify64(bits) {
  if ((bits & EXPONENT_ALL_ONES_64) !== EXPONENT_ALL_ONES_64) return null;
  const mantissa = bits & MANTISSA_64;
  if (mantissa === 0n) return null; // infinity, not NaN
  return (bits & QUIET_BIT_64) !== 0n ? "qnan64" : "snan64";
}

function classify32(bits) {
  const low = bits & 0xffffffffn;
  if ((low & EXPONENT_ALL_ONES_32) !== EXPONENT_ALL_ONES_32) return null;
  const mantissa = low & MANTISSA_32;
  if (mantissa === 0n) return null;
  return (low & QUIET_BIT_32) !== 0n ? "qnan32" : "snan32";
}

/**
 * Under `nan-insensitive`, two values match when they are bitwise equal, or
 * when both are NaN of the same signalling class at the same width. Width is
 * ambiguous from bits alone, so a match at either width is accepted.
 */
function isNanEquivalent(left, right) {
  const pairs = [
    [classify64(left), classify64(right)],
    [classify32(left), classify32(right)],
  ];
  return pairs.some(([a, b]) => a !== null && a === b);
}

export function makeComparator(policy) {
  if (!POLICIES.includes(policy)) {
    throw new Error(`unknown FP comparison policy '${policy}'`);
  }

  return function areEqual(regId, left, right) {
    if (left === right) return true;
    if (policy === "bitwise") return false;

    if (regId === RegId.FPSR) {
      if (policy !== "ignore-fpsr") return false;
      return (left & ~FPSR_CUMULATIVE_MASK) === (right & ~FPSR_CUMULATIVE_MASK);
    }
    if (regId === RegId.FPCR) return false;

    const isVector = regId >= RegId.V_BASE && regId < RegId.V_BASE + 64;
    return isVector && isNanEquivalent(left, right);
  };
}
