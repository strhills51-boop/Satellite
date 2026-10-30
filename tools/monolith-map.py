#!/usr/bin/env python3
"""Structural map of the monolith HTML (docs/semantic/monolith/structure.md).

Usage: tools/monolith-map.py <monolith.html> > docs/semantic/monolith/structure.md
Read-only. Anchors are the monolith's own section headers, so the map survives line shifts loosely;
line numbers refer to the analysed copy (sha256 printed in the header).
"""
import hashlib, re, sys
from collections import defaultdict

path = sys.argv[1]
raw = open(path, 'rb').read()
lines = raw.decode('utf-8', errors='replace').split('\n')
out = []
P = out.append

P('# Монолит — структурная карта (автоматически)')
P('')
P(f'Сгенерировано `tools/monolith-map.py`. Файл: {len(lines):,} строк, {len(raw):,} байт, sha256 `{hashlib.sha256(raw).hexdigest()[:16]}…`.'.replace(',', ' '))
P('Номера строк относятся к этой копии. Конспект по смыслу — в соседних файлах.')
P('')

# Blocks: style / script
blocks = []
stack = None
for i, l in enumerate(lines, 1):
    m = re.search(r'<(script|style)\b([^>]*)>', l)
    if m and stack is None and not re.search(r'</%s>' % m.group(1), l[m.end():]):
        attrs = m.group(2)
        idm = re.search(r'id="([^"]+)"', attrs) or re.search(r'(data-[\w-]+)', attrs)
        stack = (m.group(1), i, idm.group(1) if idm else '')
        continue
    if stack and f'</{stack[0]}>' in l:
        blocks.append((stack[0], stack[1], i, stack[2]))
        stack = None

P('## Блоки `<style>` и `<script>`')
P('')
P('| # | Тип | Строки | Объём | id |')
P('|---|---|---|---|---|')
for n, (kind, a, b, ident) in enumerate(blocks, 1):
    P(f'| {n} | {kind} | {a}–{b} | {b - a + 1} | {ident or "—"} |')
P('')

def block_of(line):
    for n, (kind, a, b, _) in enumerate(blocks, 1):
        if a <= line <= b:
            return n
    return 0

# Section headers
P('## Заголовки разделов (собственная разметка монолита `/* === … === */`)')
P('')
hdr = re.compile(r'^\s*/\* === (?!/)(.*?)(?: ===|\*/|$)')
cur = None
for i, l in enumerate(lines, 1):
    m = hdr.match(l)
    if not m:
        continue
    b = block_of(i)
    if b != cur:
        P(f'\n**Блок {b}**\n')
        cur = b
    text = m.group(1).strip().rstrip('=').strip()
    P(f'- `{i}` {text[:160]}')
P('')

# Public APIs
P('## Публичные API `window.ModlistTool*`')
P('')
apis = defaultdict(list)
for i, l in enumerate(lines, 1):
    for m in re.finditer(r'window\.(ModlistTool[A-Za-z0-9]+)\s*=(?!=)', l):
        apis[m.group(1)].append(i)
for name in sorted(apis, key=lambda k: apis[k][0]):
    ls = apis[name]
    P(f'- `{name}` — {", ".join(map(str, ls[:6]))}{" …" if len(ls) > 6 else ""}' + (f' (**назначается {len(ls)} раз**)' if len(ls) > 1 else ''))
P('')

# Overrides
P('## Переопределения функций')
P('')
P('Функция объявлена, а потом её имя переназначено (`name = function …`) или сохранено как `__orig…` и обёрнуто.')
P('')
decl = defaultdict(list)
for i, l in enumerate(lines, 1):
    m = re.match(r'^\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(', l)
    if m:
        decl[m.group(1)].append(i)
reassign = defaultdict(list)
for i, l in enumerate(lines, 1):
    m = re.match(r'^\s*([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?function\b', l)
    if m and m.group(1) in decl:
        reassign[m.group(1)].append(i)
    m = re.search(r'const\s+(__orig[\w$]*)\s*=\s*([A-Za-z_$][\w$.]*)', l)
    if m:
        target = m.group(2).split('.')[-1]
        reassign[target].append(i)
P('| Функция | Объявлена | Переопределена (строки) |')
P('|---|---|---|')
for name in sorted(reassign, key=lambda k: -len(reassign[k])):
    P(f'| `{name}` | {", ".join(map(str, decl.get(name, [])[:4])) or "—"} | {", ".join(map(str, sorted(set(reassign[name]))))} |')
P('')
dups = {k: v for k, v in decl.items() if len(v) > 1}
P(f'### Функции, объявленные больше одного раза: {len(dups)}')
P('')
for name in sorted(dups, key=lambda k: -len(dups[k]))[:80]:
    P(f'- `{name}` — {", ".join(map(str, dups[name][:8]))}')
P('')

# Runtime owners
P('## «Владельцы» v527 (behavior-preserving refactor)')
P('')
for i, l in enumerate(lines, 1):
    m = re.match(r'^\s*function (create[A-Za-z0-9]*RuntimeOwnerV527)\(', l)
    if m:
        doc = ''
        for j in range(i - 2, max(i - 12, 0), -1):
            if 'v527 behavior-preserving refactor' in lines[j]:
                doc = lines[j].split(':', 1)[-1].strip().rstrip('=').strip()
                break
        P(f'- `{i}` `{m.group(1)}` — {doc[:140]}')
P('')
print('\n'.join(out))
