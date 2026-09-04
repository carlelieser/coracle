#!/usr/bin/env bash
# Reproducible build of the guest kernel, initramfs and DTB.
#
# One entrypoint, host-agnostic: everything compiles inside a linux/arm64
# container so macOS and CI produce the same artifacts. Sources are fetched by
# pinned version and verified by SHA-256; nothing is committed.
#
#   ./build.sh          build everything into out/
#   ./build.sh clean    remove out/ and the download cache

set -euo pipefail

KERNEL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="$KERNEL_DIR/out"
CACHE_DIR="$KERNEL_DIR/.cache"

# shellcheck source=versions.env
source "$KERNEL_DIR/versions.env"

log() { printf '\n==> %s\n' "$*"; }

if [ "${1:-}" = "clean" ]; then
	log "Removing $OUT_DIR and $CACHE_DIR"
	rm -rf "$OUT_DIR" "$CACHE_DIR"
	exit 0
fi

require_docker() {
	if ! docker info >/dev/null 2>&1; then
		echo "build.sh: docker is not available or not running" >&2
		exit 1
	fi
}

# Verified download. Re-verifies a cached file so a truncated or tampered
# artifact never silently feeds the build.
fetch() {
	local url="$1" sha="$2" dest="$3"

	if [ -f "$dest" ] && echo "$sha  $dest" | shasum -a 256 -c --status 2>/dev/null; then
		echo "cached: $(basename "$dest")"
		return 0
	fi

	echo "fetching: $url"
	curl -fL --retry 3 --retry-delay 2 -o "$dest.part" "$url"

	if ! echo "$sha  $dest.part" | shasum -a 256 -c --status 2>/dev/null; then
		rm -f "$dest.part"
		echo "build.sh: SHA-256 mismatch for $url (expected $sha)" >&2
		exit 1
	fi
	mv "$dest.part" "$dest"
}

require_docker
mkdir -p "$OUT_DIR" "$CACHE_DIR"

log "Building the build container ($BUILDER_IMAGE)"
docker build --platform linux/arm64 -t "$BUILDER_IMAGE" "$KERNEL_DIR"

log "Fetching pinned sources"
fetch "$KERNEL_URL"  "$KERNEL_SHA256"  "$CACHE_DIR/linux-$KERNEL_VERSION.tar.xz"
fetch "$BUSYBOX_URL" "$BUSYBOX_SHA256" "$CACHE_DIR/busybox-$BUSYBOX_VERSION.tar.bz2"

log "Building kernel, initramfs and DTB in the container"
docker run --rm --platform linux/arm64 \
	-v "$KERNEL_DIR:/src:ro" \
	-v "$CACHE_DIR:/cache:ro" \
	-v "$OUT_DIR:/out" \
	-e "KERNEL_VERSION=$KERNEL_VERSION" \
	-e "BUSYBOX_VERSION=$BUSYBOX_VERSION" \
	"$BUILDER_IMAGE" \
	/bin/bash /src/scripts/build-in-container.sh

log "Artifacts in $OUT_DIR"
ls -la "$OUT_DIR"
cat "$OUT_DIR/manifest.txt"
