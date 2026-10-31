# Семантическая карта кода NG

Файл для агентов (Claude и других), чтобы не искать вслепую и не переопределять существующее.
Правила — PHILOSOPHY §5 «Семантический слой». Обновляется **в том же коммите**, что и код. CI проверяет,
что каждый файл `.rs` из `crates/` и `tools/` упомянут здесь (`tools/check-map.sh`).

## Понятия → единственный владелец

Перед тем как писать функцию, найди понятие здесь. Есть владелец — используй его или расширь.
Нет — добавь строку в том же коммите.

| Понятие | Владелец | Не путать с |
|---|---|---|
| Идентичность пути (регистр, сравнение) | `sat-core/src/interner.rs` (`fold`, `PathInterner`) | `sat_formats::normalize_path` — только приводит записи архивов к виду `a/b/c` |
| Суффикс разделителя `_separator` | `sat-core/src/canon.rs` (`SEPARATOR_SUFFIX`) | `sat_mo2::profile::SEPARATOR_SUFFIX` — реэкспорт, не копия |
| Папка-разделитель на диске | `sat-core/src/sync/names.rs` (`is_separator_folder`) | — |
| Имя мода для сверки (`normalizeName` монолита) | `sat-core/src/sync/names.rs` (`normalize_name`) | ключи донора `raw/norm/loose_key` там же |
| Проект — канон сборки (ADR 0006) | `sat-core/src/project.rs` (`Project`: канон + сигнатуры + записи SYNC; `observe`, `manager_diff`) | `modlist.txt` — сторона, не канон |
| Порядок, вкл/выкл, разделители (структура канона) | `sat-core/src/canon.rs` (`Modlist`) — снимок | файл `modlist.txt` — только адаптер `sat-mo2/src/profile.rs`; живое редактирование — `OrderBook` |
| Живой порядок загрузки: операции, строки вида, патчи для UI, undo/redo (ADR 0008) | `sat-core/src/order/book.rs` (`OrderBook`, `Patch`, `Row`) | `Modlist::move_entries` — та же семантика на `Vec`, для снимков и тестов |
| Декорации над каноном: цвет, логические группы | `sat-core/src/order/book.rs` (`set_color`, `create_group`) | порядок они не меняют никогда |
| Декорации как данные проекта (по именам) | `sat-core/src/order/decor.rs` (`Decor`, `GroupDef`); `OrderBook::decor` / `apply_decor`; `Project::book` / `commit_book` | — |
| Маркеры логических групп `⟦MLT-LG:v1⟧` в экспортном `modlist.txt` | `sat-core/src/order/decor.rs` (`format_marker`, `parse_marker`, `strip_markers`, `encode_markers`) | настоящий `modlist.txt` MO2 маркеров не содержит |
| Вставка, удаление, переименование записей канона (новые папки SYNC, корзина, принятый rename) | `sat-core/src/order/book.rs` (`insert_entries`, `remove_entries`, `rename`) | — |
| Позиции и строки в списке любого размера | `sat-core/src/order/seq.rs` (`Seq`: куски + деревья Фенвика) | — |
| Приоритеты из порядка | `sat-core/src/canon.rs` (`Modlist::priorities`) → `overwrite::Priorities` | — |
| Победитель пути, стеки, цепочки, контексты, W/L/M/X | `sat-core/src/overwrite.rs` (`OverwriteProjection`) | — |
| Таблица провайдеров (тёплый ярус) | `sat-core/src/table.rs` (`ProviderTable`; сборка по модам — `TableBuilder`) | — |
| Факты обхода мода | `sat-core/src/facts.rs` (`ModFacts`, `ModMetrics`) | — |
| Разбор BSA/BA2 | `sat-formats/src/archive.rs` | — |
| Заголовок DDS и VRAM | `sat-formats/src/dds.rs` | — |
| Заголовок и записи плагина | `sat-formats/src/plugin.rs` | — |
| Экземпляр MO2 (папки, профили, ini) | `sat-mo2/src/instance.rs` | — |
| Разбор и запись `modlist.txt` / `plugins.txt` / `loadorder.txt` | `sat-mo2/src/profile.rs` | — |
| `meta.ini` мода, цвет QColor | `sat-mo2/src/meta.rs`, `sat-mo2/src/ini.rs` | — |
| Атомарная запись файла | `sat-mo2/src/lib.rs` (`write_atomic`) | — |
| Сигнатура папки мода | `sat-scan/src/signature.rs` (`signature` v3 — только содержимое; `signature_v2`/`signature_v1` — только перенос кеша первого формата) | сигнатура монолита (`buildAnalysisSignature`) — другая, см. `docs/parity/run/00-overview.md` |
| Контракт анализатора, реестр, штампы версий, маршруты reuse / indexed / scan | `sat-scan/src/analyzer.rs` (`Analyzer`, `Contract`, `Registry`, `Stamp`, `Route`, `plan`) | версия формата файла кеша — `cache::CACHE_FORMAT`, не версия анализаторов |
| Кеш сканов (холодный, на машине) | `sat-scan/src/cache.rs` (конверт: сигнатура + штампы анализаторов + факты; читает и первый формат) | контейнер проекта (ADR 0007) — другое |
| Пул ввода-вывода | `sat-scan/src/lib.rs` (`scan`, `ScanOptions::io_threads`) | пул `rayon` по умолчанию — для вычислений ядра |
| Донор: соответствие, категории, оценка | `sat-core/src/sync/donor.rs` | — |
| Статусы пробега, поколения staging → authoritative / aborted, печати перед публикацией | `sat-core/src/run.rs` (`RunStatus::can_go`, `RunState`, `Authority`, `Staging`, `seal`) | — |
| Записи SYNC и сверка диска с проектом | `sat-core/src/sync/records.rs` (`reconcile`, `SyncRecord`, `Status`) | ключ сопоставления — `normalize_name`, ключ отпечатка — точное имя в нижнем регистре |

## Файлы

### `crates/sat-formats` — форматы файлов игры, без файловой системы
- `crates/sat-formats/src/lib.rs` — `FormatError`, `normalize_path`; реэкспорт модулей.
- `crates/sat-formats/src/read.rs` — чтение little-endian и z-строк (внутреннее).
- `crates/sat-formats/src/plugin.rs` — `read_plugin_header` (TES4: HEDR, CNAM, SNAM, MAST; флаги master/light/localized),
  `PluginHeader::kind`, `scan_records` → `RecordCounts` (`placed_references` = REFR + ACHR).
- `crates/sat-formats/src/dds.rs` — `DdsInfo::parse` (заголовок до `DDS_HEADER_PROBE` байт), `texture_bytes`, `dxgi_name`.
- `crates/sat-formats/src/archive.rs` — `read_archive` → `ArchiveManifest` (BSA 103–105, BA2 GNRL/DX10, VRAM записей);
  `fixtures` для тестов.

### `crates/sat-core` — ядро, без ввода-вывода
- `crates/sat-core/src/lib.rs` — оглавление слоёв.
- `crates/sat-core/src/ids.rs` — типизированные `u32`-идентификаторы (`ModId`, `PathId`, `DirId`, `NameId`, `ChainId`, `ContextId`, `ArchiveId`).
- `crates/sat-core/src/interner.rs` — `PathInterner`: арена строк, свёртка регистра, каталоги, `file_key`.
- `crates/sat-core/src/facts.rs` — факты сканера: `LooseFile`, `LooseDir`, `ArchiveFacts`, `PluginFacts`, `ModMetrics`, `ModFacts`.
- `crates/sat-core/src/table.rs` — `ProviderTable::build`: колонки провайдеров, CSR по путям и модам.
- `crates/sat-core/src/overwrite.rs` — `Priorities`, `OverwriteProjection::compute` (параллельно по путям), статистика.
- `crates/sat-core/src/project.rs` — `Project::import`, `observe` (сверка + запоминание сигнатур), `manager_diff` (видимое расхождение с `modlist.txt`), `book` / `commit_book` (живая книга с декорациями).
- `crates/sat-core/src/canon.rs` — `Modlist`, `Entry`, `EntryKind`, перемещение блоков, вкл/выкл, приоритеты, `SEPARATOR_SUFFIX`.
- `crates/sat-core/src/order/mod.rs` — оглавление движка порядка (ADR 0008).
- `crates/sat-core/src/order/seq.rs` — `Seq`: последовательность кусками, агрегаты (записи / строки / разделители), O(log n) запросы.
- `crates/sat-core/src/order/book.rs` — `OrderBook`: канон + декорации + вид, `move_keys`, `set_enabled`, `set_color`, группы, свёртка, фильтр, `rows`, undo/redo.
- `crates/sat-core/src/order/decor.rs` — `Decor` для проекта, кодек маркеров групп монолита (строгая проверка, без тихого ремонта).
- `crates/sat-core/src/order/tests.rs` — модельные тесты: зеркало UI только на патчах = вид = пересчёт с нуля.
- `crates/sat-core/src/run.rs` — таблица переходов монолита (12 статусов), `Authority` с оптимистической проверкой предка, `seal` (реестр staging, инварианты конфликтов).
- `crates/sat-core/src/sync/mod.rs` — оглавление синхронизации (ADR 0006).
- `crates/sat-core/src/sync/names.rs` — правила имён монолита (сверено с JS на 1 128 именах).
- `crates/sat-core/src/sync/donor.rs` — внешний донор: `FolderLookup`, `TargetIndex`, `validate`, `candidates`.
- `crates/sat-core/src/sync/records.rs` — `reconcile(project, disk, previous)`: записи `FsCreated / FsDeleted / FsRenamed`, решения сохраняются между прогонами.

### `crates/sat-mo2` — адаптер Mod Organizer 2
- `crates/sat-mo2/src/lib.rs` — `Mo2Error`, `read`, `write_atomic`.
- `crates/sat-mo2/src/ini.rs` — разбор ini MO2, `@ByteArray`, `@Variant` (QColor), экранирование.
- `crates/sat-mo2/src/instance.rs` — `Instance::load` (ini или раскладка `mods` + `profiles`), `from_dirs`, `mod_dir`.
- `crates/sat-mo2/src/profile.rs` — `parse_modlist` / `serialize_modlist` (байт в байт, BOM, CRLF), `plugins.txt`, `loadorder.txt`.
- `crates/sat-mo2/src/meta.rs` — `ModMeta` из `meta.ini`.

### `crates/sat-scan` — обход папок модов (холодный ярус)
- `crates/sat-scan/src/lib.rs` — `ScanJob`, `ScanOptions`, `ScanProgress` (счётчики по фазам), `scan` / `scan_batched` на I/O-пуле (стек потоков 32 МиБ).
- `crates/sat-scan/src/walk.rs` — обход одного мода: каждое наблюдение — анализаторам реестра; `reanalyze` — маршрут indexed (кешированный индекс, без обхода).
- `crates/sat-scan/src/signature.rs` — сигнатура v3 (листинг каталога, без опций), `signature_v2` / `signature_v1` для переноса кеша первого формата.
- `crates/sat-scan/src/analyzer.rs` — контракт анализатора (монолит v503), реестр, встроенные `metrics` (структурный), `dds-vram`, `plugins`, `archives` (порождает виртуальные наблюдения), выбор маршрута по штампам.
- `crates/sat-scan/src/cache.rs` — кеш по моду (bincode, временный файл + rename): формат 2 со штампами, чтение формата 1.
- `crates/sat-scan/tests/scan.rs` — интеграционный тест: DDS, плагин, архив, кеш.

### `tools`
- `tools/sat-cli/src/bench_order.rs` — `bench-order`: движок порядка на N записей (по умолчанию 1 млн).
- `tools/sat-cli/src/main.rs` — `analyze` (инвентарь, проект, скан, сверка, таблица, проекция, отчёт), `donor`, `inspect` (что видят разборщики форматов), `bench-synth`.

## Документы
- `PHILOSOPHY.md` — направление и правила. `docs/adr/` — решения. `docs/distances/` — текущая работа.
- `docs/parity/` — что делает монолит (эталон поведения). `docs/semantic/monolith/` — конспект монолита:
  `00-index.md` (области и где они), `GOLD.md` (идеи, которые переносим), `structure.md` (генерирует `tools/monolith-map.py`).
- `tools/check-map.sh` — проверка этой карты в CI. `tools/monolith-map.py` — структурная карта монолита.
