/**
 * CDT v1 wire constants and register naming.
 * Normative description: tests/TRACE_FORMAT.md
 */

export const MAGIC = "CORACLE\x01";
export const FORMAT_VERSION = 1;
export const FILE_HEADER_BYTES = 80;
export const RECORD_HEADER_BYTES = 8;
export const REG_DELTA_BYTES = 16;

export const PRODUCER = { 1: "qemu-tcg-plugin", 2: "coracle" };

export const RecordType = {
  BLOCK: 1,
  EXCEPTION: 2,
  MARKER: 3,
  END: 4,
};

export const StreamFlags = {
  PRECISE_FP: 1n << 0n,
  HAS_VREGS: 1n << 1n,
  HAS_SYSREGS: 1n << 2n,
  BLOCK_DELTAS: 1n << 3n,
};

export const RegId = {
  SP: 31,
  PC: 32,
  PSTATE: 33,
  FPCR: 34,
  FPSR: 35,
  V_BASE: 64,
  SYS_BASE: 256,
};

export const NUM_GPR = 31;
export const NUM_VREG = 32;
export const NUM_SYSREG = 20;

export const SYSREG_NAMES = [
  "SCTLR_EL1",
  "TTBR0_EL1",
  "TTBR1_EL1",
  "TCR_EL1",
  "MAIR_EL1",
  "VBAR_EL1",
  "ESR_EL1",
  "FAR_EL1",
  "ELR_EL1",
  "SPSR_EL1",
  "SP_EL0",
  "SP_EL1",
  "TPIDR_EL0",
  "TPIDR_EL1",
  "TPIDRRO_EL0",
  "CONTEXTIDR_EL1",
  "CPACR_EL1",
  "AMAIR_EL1",
  "PAR_EL1",
  "CNTKCTL_EL1",
];

export const DISCON_NAMES = { 1: "interrupt", 2: "exception", 4: "hostcall" };

export function regName(id) {
  if (id < NUM_GPR) return `x${id}`;
  if (id === RegId.SP) return "sp";
  if (id === RegId.PC) return "pc";
  if (id === RegId.PSTATE) return "pstate";
  if (id === RegId.FPCR) return "fpcr";
  if (id === RegId.FPSR) return "fpsr";
  if (id >= RegId.V_BASE && id < RegId.V_BASE + NUM_VREG * 2) {
    const index = id - RegId.V_BASE;
    return `v${index >> 1}.${index & 1 ? "hi" : "lo"}`;
  }
  if (id >= RegId.SYS_BASE && id < RegId.SYS_BASE + NUM_SYSREG) {
    return SYSREG_NAMES[id - RegId.SYS_BASE];
  }
  return `reg${id}`;
}

/** True for register ids whose comparison is governed by the FP policy. */
export function isFpReg(id) {
  if (id === RegId.FPCR || id === RegId.FPSR) return true;
  return id >= RegId.V_BASE && id < RegId.V_BASE + NUM_VREG * 2;
}

export function hex(value) {
  return `0x${value.toString(16).padStart(16, "0")}`;
}

export function decodePstate(value) {
  const nzcv = Number((value >> 28n) & 0xfn);
  const daif = Number((value >> 6n) & 0xfn);
  const el = Number((value >> 2n) & 0x3n);
  const spsel = Number(value & 1n);
  const flags = ["N", "Z", "C", "V"].filter((_, i) => nzcv & (8 >> i)).join("") || "-";
  const masks = ["D", "A", "I", "F"].filter((_, i) => daif & (8 >> i)).join("") || "-";
  return `${flags} DAIF=${masks} EL${el}${spsel ? "h" : "t"}`;
}
