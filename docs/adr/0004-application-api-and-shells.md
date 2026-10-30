# ADR 0004 — Транспорт-независимый API приложения и несколько оболочек

- Статус: **принято** (2026-10-30)
- Связано: ADR 0001, PHILOSOPHY §5 «Реактивность», прототип Satellite (command runtime, events)

## Контекст

Ядро должно работать в трёх местах: нативное окно (Tauri), headless/dev-сервер (тесты, CI, MCP),
в будущем — воркер в браузере (WASM). Интерфейс один.

## Решение

1. Слой `sat-app` описывает **весь** API приложения как три вида сообщений:
   - **Command** — намерение изменить состояние (`MoveEntries`, `SetEnabled`, `StartRun`, `ApplyPlan`…);
   - **Query** — чтение проекции страницей (`ModlistView`, `ChainFiles{offset,limit}`, `ToothTable{window}`…);
   - **Event** — уведомление о смене поколения/прогрессе (`ProjectionChanged{gen, scopes}`, `JobProgress`…).
2. Типы сообщений сериализуются (serde) и экспортируются в TypeScript (`specta`/`ts-rs`), фронт не пишет их руками.
3. Оболочки — тонкие адаптеры транспорта, без логики:
   - `apps/desktop` — Tauri IPC;
   - `apps/devserver` — HTTP (команды, запросы) + WebSocket (события);
   - будущее: `apps/web-worker` — `postMessage` в WASM-воркер.
4. UI никогда не держит свою копию истины: получил `ProjectionChanged` с номером поколения → перезапросил видимое.
5. Каждая команда проходит единый конвейер (как в прототипе Satellite):
   `dispatch → policy (возможности) → mutation слоя → invalidation → новое поколение → event`.
   Команды не рендерят и не трогают UI.

## Последствия

- Playwright-тесты паритета гоняются через dev-сервер на Linux.
- Будущий MCP-сервер — ещё одна оболочка поверх тех же Command/Query.
