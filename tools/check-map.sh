#!/usr/bin/env bash
# Semantic layer guard: see tools/check-map.py (ADR 0010 §«Семантический слой»).
exec python3 "$(dirname "$0")/check-map.py"
