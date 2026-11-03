#!/bin/bash
set -euo pipefail
cd -- "$(dirname -- "$0")"
mkdir -p build/module-cache
xcrun swiftc -O -target "$(uname -m)-apple-macosx11.0" \
  -module-cache-path "$PWD/build/module-cache" \
  -framework Foundation -framework CoreGraphics \
  main.swift -o build/macos-command-click
xcrun swiftc -O -target "$(uname -m)-apple-macosx11.0" \
  -module-cache-path "$PWD/build/module-cache" \
  -framework Foundation -framework ApplicationServices \
  repair-click-count.swift -o build/repair-click-count
printf '%s\n' "$PWD/build/macos-command-click"
printf '%s\n' "$PWD/build/repair-click-count"
