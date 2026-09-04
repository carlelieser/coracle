//! Owned by the integer slice.

use super::super::instruction::{unallocated, Form, Instruction};
use super::super::op::Op;
use super::super::operand::{Cond, RegWidth};
use super::{bits, sign_extend};
use crate::reg::Gpr;

/// `op0 = 101x` — branches, exception generation and system instructions.
///
/// Owned by the integer slice. Dispatch follows the ARM ARM's "Branches,
/// Exception Generating and System instructions" table: `op0` is bits 31..29
/// and `op1` bits 25..22.
pub fn branches_exceptions_system(encoding: u32) -> Instruction {
    let op0 = bits(encoding, 31, 29);
    let op1 = bits(encoding, 25, 22);

    match (op0, op1) {
        (0b010, 0b0000..=0b0111) => conditional_branch(encoding),
        (0b110, 0b0000..=0b0011) => exception(encoding),
        (0b110, 0b0100) => system(encoding),
        (0b110, 0b1000..=0b1111) => branch_register(encoding),
        // Bit 31 is the link bit and bits 30..29 are zero for the immediate
        // branch, so both `op0 = 000` and `op0 = 100` land here.
        (0b000 | 0b100, _) => branch_immediate(encoding),
        (0b001 | 0b101, 0b0000..=0b0111) => compare_and_branch(encoding),
        (0b001 | 0b101, _) => test_and_branch(encoding),
        // op0 = 011 and 111 are the SVE and unallocated halves of the table;
        // op0 = 010 with op1 = 1xxx is unallocated in the base architecture.
        _ => unallocated(encoding),
    }
}

/// `B` and `BL`, whose 26-bit offset covers ±128 MiB.
fn branch_immediate(encoding: u32) -> Instruction {
    let op = if bits(encoding, 31, 31) == 1 {
        Op::Bl
    } else {
        Op::B
    };
    let offset = sign_extend(bits(encoding, 25, 0), 26) * 4;
    Instruction::new(encoding, op, Form::Branch { offset })
}

/// `B.cond` and `BC.cond`.
fn conditional_branch(encoding: u32) -> Instruction {
    // `o1` (bit 24) selects `BC.cond`, which is FEAT_HBC and not advertised.
    // `o0` (bit 4) must be zero.
    if bits(encoding, 24, 24) != 0 || bits(encoding, 4, 4) != 0 {
        return unallocated(encoding);
    }
    let offset = sign_extend(bits(encoding, 23, 5), 19) * 4;
    let cond = Cond::from_bits(bits(encoding, 3, 0) as u8);
    Instruction::new(encoding, Op::B, Form::BranchCond { offset, cond })
}

/// `CBZ` and `CBNZ`.
fn compare_and_branch(encoding: u32) -> Instruction {
    let op = if bits(encoding, 24, 24) == 1 {
        Op::Cbnz
    } else {
        Op::Cbz
    };
    let form = Form::BranchReg {
        rt: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        offset: sign_extend(bits(encoding, 23, 5), 19) * 4,
        // No bit is tested; the whole register is compared against zero.
        bit: 0,
    };
    Instruction::new(encoding, op, form).with_width(RegWidth::from_sf(bits(encoding, 31, 31) == 1))
}

/// `TBZ` and `TBNZ`.
///
/// The tested bit position is split across `b5` (bit 31) and `b40` (bits
/// 23..19), and `b5` doubles as the operand width: only an X register has a
/// bit 32 or above to test.
fn test_and_branch(encoding: u32) -> Instruction {
    let op = if bits(encoding, 24, 24) == 1 {
        Op::Tbnz
    } else {
        Op::Tbz
    };
    let bit = ((bits(encoding, 31, 31) << 5) | bits(encoding, 23, 19)) as u8;
    let form = Form::BranchReg {
        rt: Gpr::from_index_zr(bits(encoding, 4, 0) as u8),
        offset: sign_extend(bits(encoding, 18, 5), 14) * 4,
        bit,
    };
    Instruction::new(encoding, op, form).with_width(RegWidth::from_sf(bits(encoding, 31, 31) == 1))
}

/// `BR`, `BLR`, `RET` and `ERET`.
fn branch_register(encoding: u32) -> Instruction {
    // `op2`, bits 20..16, is fixed at all-ones for every allocated encoding
    // in this group.
    if bits(encoding, 20, 16) != 0b11111 {
        return unallocated(encoding);
    }
    let opc = bits(encoding, 24, 21);
    let op3 = bits(encoding, 15, 10);
    let rn = bits(encoding, 9, 5) as u8;
    let rm = bits(encoding, 4, 0);

    // op3 selects the pointer-authenticating variants, which are not
    // advertised (docs/machine-spec.md §2); only the plain forms have op3 = 0.
    if op3 != 0 {
        return unallocated(encoding);
    }

    match opc {
        0b0000..=0b0010 if rm != 0 => unallocated(encoding),
        0b0000 => Instruction::new(
            encoding,
            Op::Br,
            Form::BranchIndirect {
                rn: Gpr::from_index_zr(rn),
            },
        ),
        0b0001 => Instruction::new(
            encoding,
            Op::Blr,
            Form::BranchIndirect {
                rn: Gpr::from_index_zr(rn),
            },
        ),
        0b0010 => Instruction::new(
            encoding,
            Op::Ret,
            Form::BranchIndirect {
                rn: Gpr::from_index_zr(rn),
            },
        ),
        // `ERET` names no register: Rn is fixed at 11111 and Rm at 00000.
        0b0100 if rn == 0b11111 && rm == 0 => Instruction::new(encoding, Op::Eret, Form::None),
        // opc = 0101 is DRPS, 0110/0111 are the FEAT_GCS return forms, and
        // the rest is unallocated.
        _ => unallocated(encoding),
    }
}

/// Exception generation: `SVC`, `HVC`, `SMC`, `BRK`, `HLT` and the debug
/// exceptions.
fn exception(encoding: u32) -> Instruction {
    // `imm16` occupies bits 20..5; the low five bits select the variant.
    let imm = bits(encoding, 20, 5) as u64;
    let opc = bits(encoding, 23, 21);
    let op2 = bits(encoding, 4, 2);
    let ll = bits(encoding, 1, 0);
    // `op2` is fixed at zero across the whole group.
    if op2 != 0 {
        return unallocated(encoding);
    }

    let op = match (opc, ll) {
        (0b000, 0b01) => Op::Svc,
        (0b000, 0b10) => Op::Hvc,
        (0b000, 0b11) => Op::Smc,
        (0b001, 0b00) => Op::Brk,
        (0b010, 0b00) => Op::Hlt,
        // opc = 011 is TCANCEL (FEAT_TME), and 101 is the DCPS family, which
        // only exists in debug state. Neither is advertised.
        _ => return unallocated(encoding),
    };
    Instruction::new(encoding, op, Form::Imm { imm })
}

/// The `op1 = 0100` row: hints, barriers, `MSR` (immediate) and the system
/// register moves.
fn system(encoding: u32) -> Instruction {
    let l = bits(encoding, 21, 21);
    let op0 = bits(encoding, 20, 19);
    let op1 = bits(encoding, 18, 16);
    let crn = bits(encoding, 15, 12);
    let crm = bits(encoding, 11, 8);
    let op2 = bits(encoding, 7, 5);
    let rt = bits(encoding, 4, 0) as u8;

    // `op0` here is the low two bits of the architectural `op0`, and it is
    // what separates the four sub-groups of this row.
    match (op0, l) {
        // The hint, barrier and PSTATE-write space. It transfers no register,
        // so `Rt` is fixed at 11111 and anything else is unallocated.
        (0b00, 0) if rt == 0b11111 => match (op1, crn) {
            (0b011, 0b0010) => hint(encoding, crm, op2),
            (0b011, 0b0011) => barrier(encoding, crm, op2),
            // `MSR` (immediate) writes a PSTATE field named by `op1:op2`.
            // Naming the field is M2's job, so the packed encoding rides along
            // and `CRm` carries the value written.
            (_, 0b0100) => Instruction::new(
                encoding,
                Op::Msr,
                Form::System {
                    rt: Gpr::ZR,
                    sysreg: pack_sysreg(0, op1, crn, crm, op2),
                },
            ),
            _ => unallocated(encoding),
        },
        // `SYS` and `SYSL`: the cache and TLB maintenance instructions. They
        // are EL1-only and land with the MMU in M2, so this slice leaves them
        // unclaimed rather than decoding them into a system-register move.
        (0b01, _) => unallocated(encoding),
        // `MRS`/`MSR` (register). Architectural op0 is 2 or 3 for every one.
        (0b10 | 0b11, _) => {
            let op = if l == 1 { Op::Mrs } else { Op::Msr };
            Instruction::new(
                encoding,
                op,
                Form::System {
                    rt: Gpr::from_index_zr(rt),
                    sysreg: pack_sysreg(op0, op1, crn, crm, op2),
                },
            )
        }
        _ => unallocated(encoding),
    }
}

/// The hint space, `CRn = 0010`.
///
/// Every unnamed point in it is architecturally a `NOP`, not an unallocated
/// encoding, which is what lets a binary built for a later revision run here.
fn hint(encoding: u32, crm: u32, op2: u32) -> Instruction {
    let op = match (crm, op2) {
        (0b0000, 0b000) => Op::Nop,
        (0b0000, 0b001) => Op::Yield,
        (0b0000, 0b010) => Op::Wfe,
        (0b0000, 0b011) => Op::Wfi,
        // `SEV` and `SEVL` set the event register, which is what wakes a `WFE`.
        // On a single-vCPU machine (docs/machine-spec.md §1) that only ever
        // affects this core, but keeping them distinct from `NOP` means M2's
        // `WFE` needs no re-decode.
        (0b0000, 0b100) | (0b0000, 0b101) => Op::Sev,
        // Every other point in the hint space is architecturally a `NOP`
        // rather than an unallocated encoding, which is what lets a binary
        // built against a later revision run here unchanged.
        _ => Op::Nop,
    };
    Instruction::new(encoding, op, Form::None)
}

/// The barrier space, `CRn = 0011`.
fn barrier(encoding: u32, crm: u32, op2: u32) -> Instruction {
    let op = match op2 {
        // `CLREX` clears the exclusive monitor. There is nothing to clear
        // until the monitor lands in M2, but it must decode rather than fault:
        // musl's atomics emit it.
        0b010 => Op::Clrex,
        0b100 => Op::Dsb,
        0b101 => Op::Dmb,
        0b110 => Op::Isb,
        // op2 = 000 and 001 are the FEAT_XS and FEAT_WFxT variants, 011 is
        // `TCOMMIT` (FEAT_TME), and 111 is `SB` (FEAT_SB). None is advertised.
        _ => return unallocated(encoding),
    };
    // `CRm` is the barrier's domain and ordering: the interpreter needs it to
    // tell `DMB ISHST` from `DMB SY`, even though this build orders
    // everything anyway.
    Instruction::new(encoding, op, Form::Imm { imm: crm as u64 })
}

/// Packs a system register's five encoding fields into one value.
///
/// The layout is `op0:op1:CRn:CRm:op2`, most significant first — the order the
/// architecture writes them in, so a debug print reads like the manual.
/// Naming the register is M2's job.
const fn pack_sysreg(op0: u32, op1: u32, crn: u32, crm: u32, op2: u32) -> u16 {
    (((op0 & 0b11) << 14)
        | ((op1 & 0b111) << 11)
        | ((crn & 0b1111) << 7)
        | ((crm & 0b1111) << 3)
        | (op2 & 0b111)) as u16
}
