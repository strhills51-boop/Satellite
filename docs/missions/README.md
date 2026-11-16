# Миссии (харнесс ВайбГо)

Большие плохо разложенные задачи ведутся по журналу миссии — `docs/missions/<slug>.json`, состоянию
SemanticSession, перенесённому из семантического ядра раннера ВайбГо (`tools/vibe.py`, в шапке — что откуда
взято). Журнал меняется только ходами через редьюсер:

```bash
python3 tools/vibe.py dispatch docs/missions/<slug>.json          # прочитать перед ходом
python3 tools/vibe.py apply docs/missions/<slug>.json move.json   # один ход; нарушение закона — отказ, ничего не пишется
python3 tools/vibe.py answer docs/missions/<slug>.json <HRQ-id> <ответ>   # ответ владельца на вопрос миссии
```

Журнал коммитится вместе с работой, которую описывает ход. Правила работы — навык `mission-harness`.
