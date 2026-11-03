#!/bin/sh
set -eu
case "${1:-}" in
  --help|-h|--version|-V) exec /usr/local/bin/hbbs "$@" ;;
esac
# This profile migrates an existing server identity. Do not generate a new one
# when its data mount or key files are absent.
if [ ! -s id_ed25519 ] || [ ! -s id_ed25519.pub ]; then
  echo "Existing RustDesk server key pair is required in the working directory" >&2
  exit 78
fi
exec /usr/local/bin/hbbs "$@"
