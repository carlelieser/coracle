// Emission-overhead benchmark: a tight 8-instruction loop run many times.
// Deliberately branch-dense so the block rate (and therefore the trace record
// rate) is high -- this is the pessimistic case for the plugin, since cost is
// per block, not per instruction.
.text
.global _start
_start:
    movz x0, #0x2000, lsl #16    // outer iterations
    mov  x1, #0
    mov  x2, #0
outer:
    add  x1, x1, #1
    eor  x2, x2, x1
    sub  x3, x1, #1
    orr  x4, x2, x3
    subs x0, x0, #1
    b.ne outer
park:
    b    park
