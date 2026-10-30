#!/usr/bin/env bash
# Semantic layer guard (PHILOSOPHY §5): every Rust source is listed in docs/semantic/MAP.md,
# and every path the map mentions exists.
set -euo pipefail
cd "$(dirname "$0")/.."
map=docs/semantic/MAP.md
fail=0
while IFS= read -r f; do
  grep -qF "\`$f\`" "$map" || { echo "::error file=$f::not listed in $map"; fail=1; }
done < <(find crates tools -name '*.rs' | sort)
while IFS= read -r p; do
  [ -e "$p" ] || { echo "::error file=$map::mentions missing path $p"; fail=1; }
done < <(grep -oE '`(crates|tools)/[^` ]+\.rs`' "$map" | tr -d '`' | sort -u)
[ $fail -eq 0 ] && echo "semantic map OK"
exit $fail
