#!/usr/bin/env bash
# Assembles tests/corpus/*.s into flat AArch64 images QEMU can -kernel directly.
#
# No cross-toolchain is required: Apple clang (and any clang) targets
# aarch64-unknown-none out of the box, and llvm-objcopy extracts .text.
set -euo pipefail
cd "$(dirname "$0")"

OBJCOPY="${OBJCOPY:-}"
if [ -z "$OBJCOPY" ]; then
    for candidate in llvm-objcopy "$(brew --prefix llvm 2>/dev/null)/bin/llvm-objcopy" \
                     aarch64-linux-gnu-objcopy objcopy; do
        if command -v "$candidate" >/dev/null 2>&1; then
            OBJCOPY="$candidate"
            break
        fi
    done
fi
if [ -z "$OBJCOPY" ]; then
    echo "error: no objcopy found; install llvm (brew install llvm)" >&2
    exit 1
fi

mkdir -p build
for source in corpus/*.s; do
    name=$(basename "$source" .s)
    clang -target aarch64-unknown-none -c "$source" -o "build/$name.o"
    "$OBJCOPY" -O binary --only-section=.text "build/$name.o" "build/$name.bin"
    echo "built build/$name.bin ($(wc -c < "build/$name.bin" | tr -d ' ') bytes)"
done
