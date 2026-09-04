#!/usr/bin/env bash
# Fetches a chromedriver matching the installed Chrome and prints its path.
#
# Without this, wasm-bindgen-test-runner falls back to Safari and the browser
# tests fail with an opaque `driver status: signal: 9`. CI gets its driver from
# browser-actions/setup-chrome instead.
#
#   export CHROMEDRIVER="$(scripts/chromedriver.sh)"
set -euo pipefail

cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/coracle/chromedriver"

case "$(uname -s)" in
    Darwin) chrome="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
            platform=$([ "$(uname -m)" = arm64 ] && echo mac-arm64 || echo mac-x64) ;;
    Linux)  chrome="$(command -v google-chrome || command -v chromium)"
            platform=linux64 ;;
    *)      echo "unsupported platform: $(uname -s)" >&2; exit 1 ;;
esac

if [ ! -x "$chrome" ]; then
    echo "Chrome not found at '$chrome'; install Chrome or set CHROMEDRIVER yourself" >&2
    exit 1
fi

version="$("$chrome" --version | grep -oE '[0-9]+(\.[0-9]+){3}')"
driver="$cache_dir/$version/chromedriver"

if [ ! -x "$driver" ]; then
    url="https://storage.googleapis.com/chrome-for-testing-public/$version/$platform/chromedriver-$platform.zip"
    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT
    if ! curl -fsSL -o "$tmp/driver.zip" "$url"; then
        echo "no chromedriver published for Chrome $version at $url" >&2
        exit 1
    fi
    unzip -oq "$tmp/driver.zip" -d "$tmp"
    mkdir -p "$(dirname "$driver")"
    mv "$tmp/chromedriver-$platform/chromedriver" "$driver"
    chmod +x "$driver"
    xattr -d com.apple.quarantine "$driver" 2>/dev/null || true
fi

echo "$driver"
