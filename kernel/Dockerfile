# Build environment for the guest kernel and initramfs.
# linux/arm64 so the aarch64 toolchain is native — no cross-compiler needed.
FROM debian:bookworm-20250520-slim

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
      bc \
      binutils \
      bison \
      build-essential \
      bzip2 \
      cpio \
      device-tree-compiler \
      file \
      flex \
      libelf-dev \
      libssl-dev \
      xz-utils \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
