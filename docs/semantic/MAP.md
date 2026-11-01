# Семантическая карта кода NG

Файл для агентов (Claude и других): где что лежит, кто чем владеет, что с чем не путать. Правила — ADR 0010
§«Семантический слой» и PHILOSOPHY §5. Обновляется **в том же коммите**, что и код.

## Как пользоваться

1. Перед тем как писать функцию, найди понятие в таблицах ниже. Есть владелец — расширяй его. Нет — добавь
   строку и якорь в том же коммите. Обёртка «старое + новое» поверх существующей функции запрещена.
2. **Якорь** `@sat:<id>` — точка в коде: в первой строке `//!`-заголовка модуля (якорь модуля) или комментарием
   `// @sat:<id>` над ключевым типом / функцией (якорь блока). Найти: `grep -rn "@sat:order.book.move" crates`.
3. **Заголовок модуля** каждого `.rs`: первая строка — что это + якорь модуля; строка `Owns:` — чем файл владеет
   единолично; `Not here:` — где живёт близкое.
4. CI (`tools/check-map.sh` → `tools/check-map.py`): каждый `.rs` здесь упомянут и имеет заголовок с якорем и
   `Owns:`; якоря уникальны; каждый якорь кода есть в карте, каждый якорь карты есть в коде.

## Ядро и движок (ADR 0010)

| Понятие | Якорь | Не путать с |
|---|---|---|
| Виды истины (канон, декорации, факты, решения, настройки) | `@sat:engine` (`Truth`) | проекции — производное, не истина |
| Набор изменений команды | `@sat:engine.change` (`Change`, `Keys`); книга порядка отдаёт его в `order::Change::truth` | патч строк вида — `@sat:order.book.patch` |
| Граф проекций: входы, инвалидация, ленивый план | `@sat:engine.graph` (`Graph::register / apply / plan / done`) | — |
| Сессия сборки: канон и факты, связанные с графом; команды только через неё | `@sat:session` (`@sat:session.state`, `@sat:session.command`) | граф сам (`@sat:engine.graph`) |
| Перезапись по требованию: пересчёт только после изменения канона или фактов | `@sat:session.overwrite` → проекции `conflicts` (факты) и `priorities` (канон, факты) → `overwrite` (`conflicts`, `priorities` по ключам — `Input::Keyed`) | декорации и виды её не трогают |
| Статусы пробега, поколения, печати | `@sat:run.status`, `@sat:run.authority`, `@sat:run.seal` | — |
| Ресурс: манифест (данные, без кода), род, id без доверия | `@sat:resource` (`@sat:resource.manifest`, `@sat:resource.kind`, `@sat:resource.digest`) | анализатор как контракт — `@sat:scan.analyzer` |
| Источник загрузки → доверие; возможности на ресурс | `@sat:resource.source`, `@sat:resource.capability` | id-префикс `core.` ничего не даёт |
| Политика и реестр: решения Active / NeedsApproval / Blocked, зависимости | `@sat:resource.policy`, `@sat:resource.registry`, `@sat:resource.decision` | — |
| Одобрения на машине (id + версия + дайджест) | `@sat:resource.approvals` | проект одобрений не хранит |
| Команды-ресурсы: словарь `op`, встроенные, проверка возможностей при запуске | `@sat:resource.command` (`@sat:resource.command.parse`, `@sat:resource.command.builtins`, `@sat:resource.command.check`); запуск — `Session::run` | сами операции — `@sat:order.cursor.navigate`, `@sat:order.cursor.shift` |
| Клавиши-ресурсы: аккорд, раскладка по умолчанию, карта с приоритетом источника и конфликтами | `@sat:resource.hotkeys` (`@sat:resource.hotkeys.chord`, `@sat:resource.hotkeys.builtins`, `@sat:resource.hotkeys.keymap`) | — |

## Понятия → единственный владелец

| Понятие | Якорь (файл) | Не путать с |
|---|---|---|
| Идентичность пути (регистр, сравнение) | `@sat:interner.fold` (`sat-core/src/interner.rs`) | `sat_formats::normalize_path` (`@sat:formats`) — только вид `a/b/c` записей архивов |
| Суффикс разделителя `_separator` | `@sat:canon.separator-suffix` | `sat_mo2::profile::SEPARATOR_SUFFIX` — реэкспорт, не копия |
| Папка-разделитель на диске | `@sat:sync.names.separator-folder` | — |
| Имя мода для сверки (`normalizeName` монолита) | `@sat:sync.names.normalize` | ключи донора `raw/norm/loose_key` там же |
| Проект — канон сборки (ADR 0006) | `@sat:project` (`Project`) | `modlist.txt` — сторона, не канон |
| Сверка проекта с диском, запоминание сигнатур | `@sat:project.observe` | — |
| Расхождение канона с менеджером (видимое, не применяется) | `@sat:project.manager-diff` | — |
| Живая книга проекта (канон + декорации) | `@sat:project.book` | — |
| Снимок канона (сериализация) | `@sat:canon` (`Modlist`) | живое редактирование — `@sat:order.book` |
| Перемещение на снимке (`Vec`, та же семантика, что у книги) | `@sat:canon.move-entries` | `@sat:order.book.move` |
| Приоритеты из порядка (одно правило для снимка `Modlist` и живой книги `OrderBook::entries`) | `@sat:canon.priorities` → `overwrite::Priorities` | — |
| Живой порядок: операции, строки, патчи, undo/redo (ADR 0008) | `@sat:order.book` | — |
| Патч строк вида | `@sat:order.book.patch` | набор изменений истины — `@sat:engine.change` |
| Перемещение записей (блоки разделителей, группы) | `@sat:order.book.move` | — |
| Вставка, удаление, переименование записей канона | `@sat:order.book.insert`, `@sat:order.book.remove`, `@sat:order.book.rename` | — |
| Логические группы (создание, роспуск) | `@sat:order.book.group` | маркеры `⟦MLT-LG⟧` — `@sat:order.decor.markers` |
| Приглушение («не мешает») | `@sat:order.book.muted` | технический игнор `meta.ini` — `ScanOptions::ignore_root_files` (`@sat:scan`) |
| Фильтр вида | `@sat:order.book.filter` | — |
| Строки окна | `@sat:order.book.rows` | — |
| Виды одной книги: открытие, закрытие, свой фильтр / свёртка / курсор (ADR 0009) | `@sat:order.book.views`; слои видимости — `@sat:order.seq` (`add_layer`, до 64) | окно на холсте (раскладка, связи) — будущий слой пространств |
| Декорации ↔ данные проекта | `@sat:order.book.decor` (`decor`, `apply_decor`), `@sat:order.decor.data` | — |
| Маркеры групп монолита в экспортном `modlist.txt` | `@sat:order.decor.markers` | настоящий `modlist.txt` MO2 маркеров не содержит |
| Позиции и строки в списке любого размера | `@sat:order.seq` (`Seq`) | — |
| Клавиатурный курсор: навигация, выделение | `@sat:order.cursor.navigate` | — |
| Перемещение выделения с клавиатуры | `@sat:order.cursor.shift` | — |
| Структура конфликтов (только факты): спорные пути, мод → спорные пути, пути одного мода | `@sat:overwrite.conflicts` (`ConflictIndex`) | порядок здесь не участвует |
| Победитель пути, стеки, цепочки, контексты, W/L/M/X | `@sat:overwrite.projection` | — |
| Инкрементальный пересчёт перезаписи по затронутым модам; модельный тест «инкремент = полный» | `@sat:overwrite.update`, `@sat:overwrite.tests.incremental` | — |
| Таблица провайдеров (тёплый ярус) | `@sat:table.provider-table`; сборка по модам — `@sat:table.builder` | — |
| Факты обхода мода | `@sat:facts` (`ModFacts`, `ModMetrics`) | — |
| Разбор BSA/BA2 | `@sat:formats.archive` | — |
| Заголовок DDS и VRAM | `@sat:formats.dds` | — |
| Класс загрузки плагина (NG) | `@sat:formats.plugin.kind` | тип монолита для паритета — `@sat:formats.plugin.monolith-type` |
| Экземпляр MO2 (папки, профили, ini) | `@sat:mo2.instance` | — |
| `modlist.txt` / `plugins.txt` / `loadorder.txt` | `@sat:mo2.profile` | — |
| `meta.ini` мода, цвет QColor | `@sat:mo2.meta`, `@sat:mo2.ini` | — |
| Атомарная запись файла | `@sat:mo2.write-atomic` | — |
| Сигнатура папки мода | `@sat:scan.signature.v3` (`signature_v2` / `signature_v1` — только перенос кеша) | сигнатура монолита (`buildAnalysisSignature`) — другая |
| Контракт анализатора, реестр, штампы, маршруты | `@sat:scan.analyzer.contract`, `@sat:scan.analyzer.builtin`, `@sat:scan.analyzer.plan` | формат файла кеша — `@sat:scan.cache.format` |
| Кеш сканов (холодный, на машине) | `@sat:scan.cache` | контейнер проекта (ADR 0007) — другое |
| Пул ввода-вывода | `@sat:scan.io-pool` | пул `rayon` по умолчанию — для вычислений ядра |
| Скан пачками в таблицу | `@sat:scan.batched` | — |
| Перезапуск анализаторов по кешированному индексу | `@sat:scan.walk.reanalyze` | — |
| Донор: соответствие, категории, оценка | `@sat:sync.donor` | — |
| Записи SYNC и сверка диска с проектом | `@sat:sync.records.reconcile` | ключ сопоставления — `normalize_name`, ключ отпечатка — точное имя в нижнем регистре |

## Файлы

### `crates/sat-core` — ядро, без ввода-вывода
- `crates/sat-core/src/lib.rs` — `@sat:core`, оглавление слоёв.
- `crates/sat-core/src/engine.rs` — `@sat:engine`: виды истины, наборы изменений, граф проекций (ADR 0010).
- `crates/sat-core/src/ids.rs` — `@sat:ids`: типизированные `u32`-идентификаторы.
- `crates/sat-core/src/interner.rs` — `@sat:interner`: арена строк, свёртка регистра, каталоги, `file_key`.
- `crates/sat-core/src/facts.rs` — `@sat:facts`: факты сканера.
- `crates/sat-core/src/table.rs` — `@sat:table`: колонки провайдеров, CSR по путям и модам.
- `crates/sat-core/src/overwrite.rs` — `@sat:overwrite`: проекция перезаписи (параллельно по путям), статистика.
- `crates/sat-core/src/canon.rs` — `@sat:canon`: снимок канона, блоки, приоритеты.
- `crates/sat-core/src/project.rs` — `@sat:project`: проект, `observe`, `manager_diff`, `book` / `commit_book`.
- `crates/sat-core/src/run.rs` — `@sat:run`: статусы пробега, `Authority`, `seal`.
- `crates/sat-core/src/session.rs` — `@sat:session`: книга порядка + таблица фактов + граф проекций; ленивые `priorities()` / `overwrite()`.
- `crates/sat-core/src/order/mod.rs` — `@sat:order`: оглавление движка порядка (ADR 0008).
- `crates/sat-core/src/order/seq.rs` — `@sat:order.seq`: последовательность кусками, агрегаты, O(log n) запросы.
- `crates/sat-core/src/order/book.rs` — `@sat:order.book`: канон + декорации + вид, операции, патчи, история.
- `crates/sat-core/src/order/cursor.rs` — `@sat:order.cursor`: фокус / якорь / выделение и раскладка клавиш.
- `crates/sat-core/src/order/decor.rs` — `@sat:order.decor`: декорации для проекта, кодек маркеров групп.
- `crates/sat-core/src/order/tests.rs` — `@sat:order.tests`: модельные тесты.
- `crates/sat-core/src/resource/mod.rs` — `@sat:resource`: манифест, источник, возможности, политика, реестр (ADR 0011).
- `crates/sat-core/src/resource/approvals.rs` — `@sat:resource.approvals`: одобрения пользователя, файл на машине.
- `crates/sat-core/src/resource/command.rs` — `@sat:resource.command`: встроенные команды и их запуск.
- `crates/sat-core/src/resource/hotkeys.rs` — `@sat:resource.hotkeys`: аккорды, раскладка, карта клавиш.
- `crates/sat-core/src/sync/mod.rs` — `@sat:sync`: оглавление синхронизации (ADR 0006).
- `crates/sat-core/src/sync/names.rs` — `@sat:sync.names`: правила имён монолита.
- `crates/sat-core/src/sync/donor.rs` — `@sat:sync.donor`: внешний донор.
- `crates/sat-core/src/sync/records.rs` — `@sat:sync.records`: записи SYNC, сверка.

### `crates/sat-formats` — форматы файлов игры, без файловой системы
- `crates/sat-formats/src/lib.rs` — `@sat:formats`: `FormatError`, `normalize_path`, реэкспорты.
- `crates/sat-formats/src/read.rs` — `@sat:formats.read`: чтение little-endian и z-строк.
- `crates/sat-formats/src/plugin.rs` — `@sat:formats.plugin`: TES4, скан записей, класс и тип плагина.
- `crates/sat-formats/src/dds.rs` — `@sat:formats.dds`: заголовок DDS, `texture_bytes`, `dxgi_name`.
- `crates/sat-formats/src/archive.rs` — `@sat:formats.archive`: BSA 103–105, BA2 GNRL/DX10.

### `crates/sat-mo2` — адаптер Mod Organizer 2
- `crates/sat-mo2/src/lib.rs` — `@sat:mo2`: `Mo2Error`, `read`, `write_atomic`.
- `crates/sat-mo2/src/ini.rs` — `@sat:mo2.ini`: ini MO2, `@ByteArray`, `@Variant`.
- `crates/sat-mo2/src/instance.rs` — `@sat:mo2.instance`: `Instance::load`, `from_dirs`, `mod_dir`.
- `crates/sat-mo2/src/profile.rs` — `@sat:mo2.profile`: файлы профиля байт в байт.
- `crates/sat-mo2/src/meta.rs` — `@sat:mo2.meta`: `meta.ini`.

### `crates/sat-scan` — обход папок модов (холодный ярус)
- `crates/sat-scan/src/lib.rs` — `@sat:scan`: `ScanJob`, `ScanOptions`, `ScanProgress`, `scan` / `scan_batched`, выбор маршрута.
- `crates/sat-scan/src/analyzer.rs` — `@sat:scan.analyzer`: контракт, реестр, встроенные анализаторы, `plan`.
- `crates/sat-scan/src/walk.rs` — `@sat:scan.walk`: обход мода, `reanalyze`.
- `crates/sat-scan/src/signature.rs` — `@sat:scan.signature`: сигнатуры v3 / v2 / v1.
- `crates/sat-scan/src/cache.rs` — `@sat:scan.cache`: формат 2 со штампами, чтение формата 1.
- `crates/sat-scan/tests/scan.rs` — `@sat:scan.it`: интеграционный тест.

### `tools`
- `tools/sat-cli/src/main.rs` — `@sat:cli`: `analyze`, `donor`, `inspect`, `bench-synth`.
- `tools/sat-cli/src/bench_order.rs` — `@sat:cli.bench-order`: бенчмарк движка порядка.

## Документы
- `PHILOSOPHY.md` — направление и правила. `docs/adr/` — решения (0010 — ядро и движок, 0011 — реестр ресурсов). `docs/distances/` — текущая работа.
- `docs/parity/` — что делает монолит (эталон поведения). `docs/semantic/monolith/` — конспект монолита:
  `00-index.md` (области и где они), `GOLD.md` (идеи, которые переносим), `structure.md` (генерирует `tools/monolith-map.py`).
- `docs/semantic/old-satellite/` — конспект прототипа «Satellite V2 Constructor Core»: `00-conspectus.md` (по слоям; §13–14 — построчное дочитывание пробега и остальных областей,
  с пометками берём / меняем / ошибка), `BORROW.md` (что переносим и что не повторяем), `structure.md` (разделы).
- `tools/check-map.sh` → `tools/check-map.py` — проверка этой карты и якорей в CI. `tools/monolith-map.py` — структурная карта монолита.
