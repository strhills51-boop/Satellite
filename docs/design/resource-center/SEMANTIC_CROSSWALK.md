# Semantic crosswalk — pointers, not a replacement for MAP.md

| UI resource | Semantic owner / boundary | Grounding | Status |
|---|---|---|---|
| Mod tile | `@sat:order.card`, `@sat:order.decor.style`, `@sat:app.decoration` | ADR 0012; MAP.md | style/flow accepted |
| Canonical row layout | `@sat:order.flow`, `@sat:app.lines` | ADR 0012; MAP.md | accepted |
| Expanded spatial list | `@sat:order.flow.indicator-line`, `@sat:order.flow.reserve` | ADR 0016; MAP.md | mixed implementation and proposed phases — verify live |
| Selection/focus | `@sat:order.book.views`; window view boundary | ADR 0009; MAP.md | proposal portions |
| Conflict cards | `@sat:app.cards.files`, `@sat:app.cards.records` | MAP.md | available concept/API; UI proof separately |
| Conflict table | `@sat:app.context-table` | MAP.md | API anchor |
| Decoration commands | `@sat:order.book.style`, `@sat:app.patch-style` | MAP.md; ADR 0012 | accepted |
| Tags/stickers | `@sat:order.decor.marks`, `@sat:app.marks` | MAP.md | accepted |
| Counter values | `@sat:counters`, `@sat:app.counters` | MAP.md; ADR 0013 | projection, don't hardcode |
| Windows/resources | ADR 0009 / ADR 0011 / `@sat:resource` | MAP.md | 0009 is proposed; 0011 accepted, partial |
| UI control roles | ADR 0020 §1: Mode / Action / Data / Toggle | proposed UI direction | design taxonomy; no domain command owned here |
| Natural brick width | `@sat:order.flow.natural`; `@sat:app.bricks` (as recorded in ADR 0020) | ADR 0020 §6; verify current MAP | claims implementation in handoff; not a fresh verification |
| SMART color control | historical monolith V388 | UI donor only | no NG semantic anchor claimed |
| FAST, elastic, tooth | design reference + user specification | Resource center code | no NG semantic anchor claimed |

The correct route is: UI interaction → existing app command/query; never UI event → direct mutation of truth.
