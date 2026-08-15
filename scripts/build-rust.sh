#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if [ -f "$HOME/esp/esp-idf/export.sh" ]; then
    # shellcheck disable=SC1091
    . "$HOME/esp/esp-idf/export.sh"
else
    echo "ESP-IDF not found at ~/esp/esp-idf. Install it first (see README)." >&2
    exit 1
fi

cd rust-firmware
exec cargo build "$@"
