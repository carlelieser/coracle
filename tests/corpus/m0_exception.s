// Exercises the exception path: installs a vector table, then executes an
// undefined instruction so the trace carries a REC_EXCEPTION with full state.
.text
.global _start
_start:
    adr  x0, vectors
    msr  vbar_el1, x0
    isb
    mov  x1, #0xaaa
    .inst 0x00000000         // permanently UNDEFINED -> sync exception
    mov  x2, #0xbbb          // reached only via the handler's ERET
park:
    b    park

.balign 2048
vectors:
    .rept 4                  // current EL with SP0
    .balign 128
    b    .
    .endr
    .balign 128              // current EL with SPx, synchronous
sync_spx:
    mrs  x10, esr_el1
    mrs  x11, elr_el1
    add  x11, x11, #4        // skip the undefined instruction
    msr  elr_el1, x11
    eret
