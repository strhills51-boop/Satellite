#!/usr/bin/env python3
"""Semantic layer guard (ADR 0010 §«Семантический слой», PHILOSOPHY §5).

Checks, failing CI on any violation:
1. every Rust source under crates/ and tools/ is listed in docs/semantic/MAP.md;
2. every source path the map mentions exists;
3. every Rust source starts with a `//!` header whose first line carries a module anchor `@sat:<id>`
   and which has an `Owns:` line;
4. anchors are defined once across the repository (definition = the anchor on a module's first `//!` line or a
   line that is only `// @sat:<id>`); any other mention is a reference and must point to a defined anchor;
5. every anchor in code is mentioned in the map, and every anchor the map mentions exists in code.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
MAP = os.path.join(ROOT, "docs", "semantic", "MAP.md")
ANCHOR = re.compile(r"@sat:([a-z0-9][a-z0-9._-]*[a-z0-9])")
BLOCK = re.compile(r"^//\s*@sat:[a-z0-9][a-z0-9._-]*[a-z0-9]\s*$")


def sources():
    for top in ("crates", "tools"):
        for dirpath, dirnames, files in os.walk(os.path.join(ROOT, top)):
            dirnames[:] = [d for d in dirnames if d not in ("target", "node_modules")]
            for f in files:
                if f.endswith(".rs"):
                    yield os.path.relpath(os.path.join(dirpath, f), ROOT).replace(os.sep, "/")


def error(path, msg):
    print(f"::error file={path}::{msg}")


def main():
    fail = False
    text = open(MAP, encoding="utf-8").read()
    files = sorted(sources())

    for f in files:
        if f"`{f}`" not in text:
            error(f, "not listed in docs/semantic/MAP.md")
            fail = True
    for p in sorted(set(re.findall(r"`((?:crates|tools)/[^` ]+\.rs)`", text))):
        if not os.path.exists(os.path.join(ROOT, p)):
            error("docs/semantic/MAP.md", f"mentions missing path {p}")
            fail = True

    code_anchors = {}
    references = []
    for f in files:
        lines = open(os.path.join(ROOT, f), encoding="utf-8").read().split("\n")
        header = []
        for line in lines:
            if line.startswith("//!"):
                header.append(line)
            elif header or line.strip():
                break
        if not header or not ANCHOR.search(header[0]):
            error(f, "first `//!` line must name the module and carry its anchor `@sat:<id>`")
            fail = True
        if not any(l.startswith("//! Owns:") for l in header):
            error(f, "module header needs an `//! Owns:` line (what this file alone owns)")
            fail = True
        for i, line in enumerate(lines, 1):
            stripped = line.strip()
            if not stripped.startswith("//"):
                continue
            # a definition: the anchor on the first header line, or a line that is only `// @sat:<id>`
            definition = (i == 1 and header) or BLOCK.match(stripped)
            for a in ANCHOR.findall(line):
                if not definition:
                    references.append((a, f, i))
                    continue
                if a in code_anchors:
                    error(f, f"anchor @sat:{a} also defined in {code_anchors[a][0]}:{code_anchors[a][1]} (line {i})")
                    fail = True
                else:
                    code_anchors[a] = (f, i)

    for a, f, i in references:
        if a not in code_anchors:
            error(f, f"line {i} refers to @sat:{a}, which is not defined anywhere")
            fail = True

    map_anchors = set(ANCHOR.findall(text))
    for a, (f, i) in sorted(code_anchors.items()):
        if a not in map_anchors:
            error(f, f"anchor @sat:{a} (line {i}) is not in docs/semantic/MAP.md")
            fail = True
    for a in sorted(map_anchors - set(code_anchors)):
        error("docs/semantic/MAP.md", f"anchor @sat:{a} does not exist in code")
        fail = True

    if not fail:
        print(f"semantic map OK: {len(files)} sources, {len(code_anchors)} anchors, {len(references)} references")
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main())
