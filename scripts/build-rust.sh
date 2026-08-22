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

# esp-idf-sys's bindgen step needs espup's esp-clang, which clang-sys does not
# find on its own. Locate it under the active `esp` rustup toolchain instead
# of hardcoding a path, so this works on any machine that ran `espup install`.
if [ -z "${LIBCLANG_PATH:-}" ]; then
    esp_toolchain_root="$(rustup toolchain list -v 2>/dev/null | awk '$1 == "esp" {print $NF}')"
    if [ -n "$esp_toolchain_root" ]; then
        for candidate in "$esp_toolchain_root"/xtensa-esp32-elf-clang/*/esp-clang/lib; do
            if [ -d "$candidate" ]; then
                export LIBCLANG_PATH="$candidate"
                break
            fi
        done
    fi
fi

if [ -z "${LIBCLANG_PATH:-}" ]; then
    echo "Could not locate esp-clang's libclang under the 'esp' rustup toolchain." >&2
    echo "Install it with 'espup install', or set LIBCLANG_PATH manually." >&2
    exit 1
fi

cd rust-firmware
exec cargo build "$@"
