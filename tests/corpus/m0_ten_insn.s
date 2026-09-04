// M0 gate program: exactly 10 architecturally-visible instructions, then a
// self-branch park. Touches GPR moves, ALU ops, flag-setting and a conditional
// select so the trace exercises PSTATE as well as the register file.
.text
.global _start
_start:
    mov  x0, #1              // 1
    mov  x1, #2              // 2
    add  x2, x0, x1          // 3   x2 = 3
    sub  x3, x2, x0          // 4   x3 = 2
    lsl  x4, x2, #4          // 5   x4 = 0x30
    orr  x5, x4, x3          // 6   x5 = 0x32
    eor  x6, x5, x5          // 7   x6 = 0
    subs x7, x2, x1          // 8   x7 = 1, sets NZCV
    csel x8, x0, x1, eq      // 9   x8 = x1 (Z clear)
    mvn  x9, x6              // 10  x9 = ~0

park:
    b    park
