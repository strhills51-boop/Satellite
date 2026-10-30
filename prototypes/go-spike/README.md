# ModlistTool v2 (Monolith engine)

Локальный инструмент для больших сборок Mod Organizer 2. Один `modlisttool.exe`, интерфейс открывается в браузере
на `http://127.0.0.1:7317/`. Ничего не отправляет в сеть.

## Запуск (Windows)
1. Положи `modlisttool.exe` в любую папку (рядом он создаст `modlisttool.json` с последним инстансом).
2. Запусти — откроется браузер. Укажи папку инстанса MO2 (где лежит `ModOrganizer.ini`) и, если моды лежат
   ещё где-то, дополнительные папки модов.
3. «Анализ mods» → «Запустить пробег». Первый проход читает всё; дальше работает кеш по сигнатурам.

Данные проекта: `<profile>/monolith/` — `project.json` (теги, заметки, цвета, решённые цепочки), `cache/` (сканы),
`versions/` (бэкапы modlist.txt перед каждой записью).

**Закрой MO2 перед сохранением modlist.txt** — MO2 перезаписывает файл при выходе.

Флаги: `-addr 127.0.0.1:7317`, `-no-browser`, `-open <инстанс>`, `-extra <папка модов>`, `-scan`,
`-synth <папка>` (сгенерировать тестовый инстанс).

## Сборка
`./build.sh` — нужен Go 1.24+ и Node 20+. Тесты: `go test ./...`, бенчмарк ядра:
`BENCH=1 BENCH_MODS=5000 BENCH_FILES=1100 go test ./internal/index -run Bench -v`.

Структура: `internal/{mo2,scan,formats,index,engine,server}`, UI — `web/` (Svelte 5 + Vite). Спецификация и замеры — `docs/SPEC.md`.
