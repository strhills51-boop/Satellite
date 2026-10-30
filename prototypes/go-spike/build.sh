#!/usr/bin/env bash
# Builds the web UI and the Windows/Linux binaries into ./dist
set -euo pipefail
cd "$(dirname "$0")"
(cd web && npm install --no-audit --no-fund >/dev/null && npx vite build)
mkdir -p dist
VER=${VER:-0.1.0}
LD="-s -w -X main.version=$VER"
GOOS=windows GOARCH=amd64 go build -trimpath -ldflags "$LD" -o dist/modlisttool.exe ./cmd/modlisttool
GOOS=linux   GOARCH=amd64 go build -trimpath -ldflags "$LD" -o dist/modlisttool ./cmd/modlisttool
ls -la dist
