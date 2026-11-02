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
| API приложения: команды, запросы, события поверх сессии, без транспорта (ADR 0004) | `@sat:app` (`@sat:app.command`, `@sat:app.query`, `@sat:app.event`, `@sat:app.dispatch`) | транспорт — оболочки (`apps/`) |
| Стиль карточки (контейнер записи, ADR 0012): раскладка и вид, интернирование, отмена, в проекте | `@sat:order.decor.style` (`CardStyle`), `@sat:order.book.style` (`set_style`, `Op::Style`), `@sat:order.book.patch-style` (одно поле на многих, один шаг отмены), команды `SetStyle` / `PatchStyle` (`@sat:app.patch-style`, merge patch), редактор — `@sat:app.decoration` (`Query::Decoration`: выделение и его стили для «mixed») | цвет записи — `set_color` |
| Теги, стикеры, заметка записи (ручной слой; найденные анализом — отдельный слой фактов) | `@sat:order.decor.marks` (`Marks`, нормальная форма), `@sat:order.book.marks` (`patch_marks`, `Op::Marks`, `tags_in_use`); команда окна — `@sat:app.marks` (`EditMarks`, `Query::Tags`) | в проекте — `Decor::marks` |
| Каталог тегов: коллекции («Системные теги», «Стикеры», свои), цвет тега — явный, иначе по смысловой группе | `@sat:tags`, `@sat:tags.catalog`, `@sat:tags.color` (`group_color`); в проекте — `tags`; окно — `Query::Tags` (`TagView`: имя, записей, цвет) | какие теги у записи — `@sat:order.decor.marks` |
| Правила как данные (ADR 0013): метрики мода и язык выражений — для поиска, условных стикеров, кейсов счётчиков, правил тегов | метрики — `@sat:metrics`, `@sat:metrics.catalog`, `@sat:metrics.value` (`ModView`), синонимы — `@sat:metrics.aliases` (`resolve`); язык — `@sat:expr`, `@sat:expr.compile`, `@sat:expr.eval`; поиск в строке — `@sat:cli.find` (`analyze --find`) | откуда факты — скан (`ModMetrics`), перезапись (`ModStat`), канон |
| Теги по наборам правил (найденные анализом; детекторы монолита — файлом `rules/tags/monolith-builtin.json`) | `@sat:tag-rules` (формат, сопоставление монолита), `@sat:tag-rules.set` (`RuleSet`, `BUILTIN`), `@sat:tag-rules.apply` (`Compiled::tags`, Aho-Corasick по полям); в строке — `@sat:cli.tags` (`analyze --tags`, `--tag-rules FILE`), метрика `tags` для `--find` | ручные теги — `@sat:order.decor.marks`; цвета — `@sat:tags` |
| Найденные теги в сессии, условные стикеры, поиск в окне (ADR 0013 шаг 3) | найденные — `ModSummary::found_tags` (`@sat:table.summary`, `TableBuilder::with_tag_rules`); стикеры — `@sat:sticker-rules`, `@sat:sticker-rules.compile`, системные (`planned`, `new`) — `@sat:project.system-stickers`, раздел проекта `sticker_rules`, команда — `@sat:cli.stickers`; вид мода и поиск — `@sat:app.find`, `@sat:app.find.view`, `@sat:app.find.query` (`Query::Find`); в карточке — раздел `rules` (`@sat:app.sheet`) | показ в окне — с владельцем |
| Раскладка потоком (ADR 0012 §2): блоки разделителей, ряды карточек, окно рядов по высоте, ряд фокуса | `@sat:order.flow` (`FlowParams`, `flow_lines`), `@sat:order.flow.index` (`flow_index`), `@sat:order.flow.row-y` (`flow_row_y`), охват блоков (`@sat:order.flow.scope`, `flow_index_in`: классический вид монолита — текущий разделитель или выбранные стопкой), запрос страницы `@sat:app.lines` (`Query::Lines`) | стиль как данные — `@sat:order.decor.style` |
| Внешний вид цепочки и контекста: эмодзи и оттенок из состава (не из порядка) | `@sat:identity` (`@sat:identity.of`) | сами цепочки — `@sat:overwrite.projection` |
| Игнор-лист конфликтов (монолит `conflictIgnorePaths`): `meta.ini` — файл где угодно, `docs/` — папка где угодно, `a/b.dds` — один путь | `@sat:ignore`, `@sat:ignore.matches`; применение — `ConflictIndex::build_with` (`@sat:overwrite.conflicts`); в сессии — `@sat:session.ignore` (истина Settings); в проекте — `conflict_ignore`; команда окна `SetConflictIgnore`; строка — `@sat:cli.ignore` | проверяются только оспариваемые пути |
| Задачи (долгая работа): отмена, поколения, вытеснение новой задачей того же рода, единые ворота публикации результата | `@sat:jobs`, `@sat:jobs.supervisor` (`Jobs::start`, `cancel`), `@sat:jobs.publish`; отмена скана между пачками — `@sat:scan.cancel` (`ScanProgress::cancel`, `Completion`) | сама работа — скан, правила; применение результата — сессия |
| Нормализованные цепочки (слой монолита v499–v501): точные цепочки одного победителя → одна, супер-маршрут, без циклов, капы 256 / 128 | `@sat:routes` (`RouteGroups`, `OverwriteProjection::routes`), `@sat:routes.groups`, `@sat:routes.of-mod`; заголовки и «решено» — `@sat:app.chains`, `@sat:app.chains.heads`; бейджи — `@sat:app.badges.normalized` | точные стеки — `@sat:overwrite.projection`; решения — на точных (`@sat:decisions`) |
| Бейджи перезаписи на карточке (модель «коллекция цепочек» монолита): по контексту, значки крупнейших цепочек, `+N` | `@sat:app.badges` (`Query::Rows { badges }`) | модель `OW N` — та же выдача, другой показ на странице |
| Таблица конфликтов контекста (монолит, флажок «цветные типы»): моды × файлы окнами по обеим осям, группы по разделителям, порядок «цепочка / сила» | `@sat:app.context-table` (`Query::Contexts`), `@sat:app.context-table.window` (`Query::ContextTable`) | цепочки и стеки — `@sat:overwrite.projection`; цвет типа файла — страница |
| Решения по конфликтам: цепочка «решена» на подписи содержимого, переживает перестановку, пробег и переименование мода | `@sat:decisions` (`@sat:decisions.chains`), в сессии `@sat:session.decide`, команда `SetChainDone`; ключи участников — id сущностей: `@sat:session.member-keys` | хранение — раздел `decisions` проекта (`@sat:project`) |
| Сводка мода: с кем и в чём конфликтует (соперники ▲/▼, «зубы» по цепочкам, цепочки с путями), плагины мода | `@sat:app.mod-detail` (`Query::ModDetail`) | счётчики W/L/M на карточке — `Counters` |
| Карточка мода — стандарт быстрого обзора (ADR 0015): значок / мини / полная, разделы с происхождением (наблюдено, пользователь; позже автор, сообщество) и сигналами | `@sat:app.sheet` (`ModSheet`, `Query::ModSheet`), сборка — `@sat:app.sheet.build`; сводка фактов мода без путей — `@sat:table.summary` | подписи полей — в окне, по id поля |
| Фокус по ссылке («перейти к моду»), раскрывая свёрнутый блок | `@sat:order.cursor.focus-entry` (`Command::Focus`) | навигация клавишами — `@sat:order.cursor.navigate` |
| Оболочка разработки: сборка MO2 в браузере на localhost, песочница | `@sat:devserver`, `@sat:devserver.load`, `@sat:devserver.http` | смысл команд — `@sat:app` |
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
| Канон плагинов: `loadorder.txt` + отметки `plugins.txt` (своё пространство ключей, `Truth::PluginCanon`) | `@sat:plugins.canon` | порядок модов — `@sat:canon.priorities` |
| Версии плагина: одно имя в нескольких модах («ловушка версионности» монолита) | `@sat:plugins.versions` | — |
| Список плагинов: победившая версия, состояние, индекс `00…FD` / `FE:000…FFF`, переполнение, недостающие мастера, цвет | `@sat:plugins.list`; в сессии — `@sat:session.plugins` | чтение заголовка — `@sat:formats.plugin.kind` |
| Таблица провайдеров (тёплый ярус) | `@sat:table.provider-table`; сборка по модам — `@sat:table.builder`; остальное о моде (метрики, нет папки, ошибка скана) — `@sat:table.summary` | — |
| Факты обхода мода | `@sat:facts` (`ModFacts`, `ModMetrics`) | — |
| Разбор BSA/BA2 | `@sat:formats.archive` | — |
| Заголовок DDS и VRAM | `@sat:formats.dds` | — |
| Класс загрузки плагина (NG) | `@sat:formats.plugin.kind` | тип монолита для паритета — `@sat:formats.plugin.monolith-type` |
| Экземпляр MO2 (папки, профили, ini) | `@sat:mo2.instance` | — |
| `modlist.txt` / `plugins.txt` / `loadorder.txt` | `@sat:mo2.profile`; два формата `plugins.txt` (со звёздами / список без звёзд = все активны) — `@sat:mo2.plugins-txt`, запись в формате прежнего файла — `@sat:mo2.plugins-write` | порядок — `loadorder.txt`; плагины сверх лимита MO2 в списки не вносит — их находит скан |
| Правка плагинов: отметка, скрытие (blacklist), каскадное включение по мастерам, что получают `plugins.txt` / `loadorder.txt` | `@sat:plugins.edit` (`set_checked`, `set_blacklisted`), `@sat:plugins.edit.cascade`, `@sat:plugins.edit.files`; скрытые в списке — `Plugin::blacklisted` (`@sat:plugins.list`); в проекте — `plugin_blacklist` | команды окна — `@sat:app.plugins-edit` (`SetPluginChecked` включает мод-владельца, `SetPluginHidden`, `CascadePlugins`; пока без отмены); запись при фиксации — `Payload::plugin_files` (`@sat:mo2.commit`) |
| `meta.ini` мода, цвет QColor | `@sat:mo2.meta`, `@sat:mo2.ini` | — |
| Атомарная запись файла | `@sat:mo2.write-atomic` | — |
| Сигнатура папки мода | `@sat:scan.signature.v3` (`signature_v2` / `signature_v1` — только перенос кеша) | сигнатура монолита (`buildAnalysisSignature`) — другая |
| Контракт анализатора, реестр, штампы, маршруты | `@sat:scan.analyzer.contract`, `@sat:scan.analyzer.builtin`, `@sat:scan.analyzer.plan` | формат файла кеша — `@sat:scan.cache.format` |
| Кеш сканов (холодный, на машине) | `@sat:scan.cache` | контейнер проекта (ADR 0007) — другое |
| Пул ввода-вывода | `@sat:scan.io-pool` | пул `rayon` по умолчанию — для вычислений ядра |
| Скан пачками в таблицу | `@sat:scan.batched` | — |
| Перезапуск анализаторов по кешированному индексу | `@sat:scan.walk.reanalyze` | — |
| Донор: соответствие, категории, оценка | `@sat:sync.donor` | — |
| Устойчивые id сущностей (ADR 0007): имя — атрибут; переименование не теряет декорацию, метки, подпись | `@sat:entity` (`EntityId`, `@sat:entity.mint`), `@sat:entity.registry` (`Entities`: id ↔ имя, `follow`), `@sat:entity.rename`; проект — схема 2, разделы по id: `@sat:project.load` (миграция 1 → 2), `@sat:project.apply-plan`; перекладка ключей декорации — `@sat:order.decor.rekey` | книга порядка внутри сессии — по именам и `Key`; перевод на границе проекта |
| Факты модов в проекте (холодный ярус, ADR 0007 шаг 2): проект работает без сборки | `@sat:stll`, формат факта мода — `@sat:stll.facts.format` (`encode`/`decode`); в файле — `@sat:stll.file.facts` (чтение по id лениво), `@sat:stll.file.put` (неизменившиеся копируются как есть); ранняя раскладка-папка (`FactsStore`, `@sat:stll.facts`, `@sat:stll.facts.put`, `@sat:stll.facts.get`) — только для перевода; запись после скана и `--offline` — `@sat:cli.facts`; окно без сборки — `@sat:devserver.offline` (`--project`) | кеш скана машины — `@sat:scan.cache` |
| Файл проекта `.stll` (ADR 0007 шаг 3): один zip — манифест, разделы NDJSON, факты; атомарное сохранение, чужое сохраняется | `@sat:stll.file` (`Stll`), `@sat:stll.file.version`, `@sat:stll.file.open`, `@sat:stll.file.save` (`Writer`: `put_facts`, `finish`, `finish_to`); разрезание дерева проекта — `@sat:stll.sections`, `@sat:stll.sections.split`, `@sat:stll.sections.join`; неизвестные поля проекта — `Project::extra` (`@sat:project`); перевод ранней раскладки — `@sat:stll.migrate`; команды — `@sat:cli.project` | замена проекта в фиксации — `@sat:mo2.commit.project` |
| Записи SYNC и сверка диска с проектом | `@sat:sync.records.reconcile` | ключ сопоставления — `normalize_name`, ключ отпечатка — точное имя в нижнем регистре |
| Фиксация (техпроцесс «скан/синхронизация → обработка → фиксация», NG-D003 §7): решения по записям, план, бэкап, журнал, откат | ответы — `@sat:sync.records.resolution`; план — `@sat:sync.plan`, `@sat:sync.plan.build`, `@sat:sync.plan.canon`, `@sat:sync.plan.order`; исполнение — `@sat:mo2.commit`, `@sat:mo2.commit.execute`, `@sat:mo2.commit.undo`; API приложения — `@sat:app.sync`, `@sat:app.sync.decide` (`Command::Decide`, `Query::Sync`: записи, ответы, план; исполнения нет); командная строка — `@sat:cli.commit`, `@sat:cli.commit.sync`, `@sat:cli.commit.run` | сверка — `@sat:sync.records.reconcile`; запись файлов — `@sat:mo2.write-atomic` |
| Намерения и запланированные моды (ADR 0007 шаг 4): мод в порядке раньше, чем на диске; ставится фиксацией из архива загрузок | запись — `@sat:intent`, `@sat:intent.record` (`Source::Downloads`); запланированные — `@sat:intent.planned` (`Project::planned`, `plan_install`; папки нет — не расхождение); в плане — `@sat:sync.plan.installs` (`DiskOp::Install`, `Blocker::SourceUnavailable`); установка — `@sat:mo2.install`, `@sat:mo2.install.layout` (корень `Data`), `@sat:mo2.install.fomod` (выбор установщика → файлы), формат FOMOD — `@sat:formats.fomod`, `@sat:formats.fomod.parse`, `@sat:formats.fomod.resolve`, `@sat:mo2.install.facts` (факты до установки), `@sat:mo2.install.extract`; в фиксации — `@sat:mo2.commit.install` (откат удаляет распакованное); команда — `@sat:cli.intent` (`plan-install`), проверка источника — `@sat:cli.intent.source` | rar — далее; условия FOMOD о плагинах игры считаются невыполненными; адаптеры площадок — позже |

## Файлы

### `crates/sat-core` — ядро, без ввода-вывода
- `crates/sat-core/src/lib.rs` — `@sat:core`, оглавление слоёв.
- `crates/sat-core/src/decisions.rs` — `@sat:decisions`: решённые цепочки по подписи содержимого.
- `crates/sat-core/src/engine.rs` — `@sat:engine`: виды истины, наборы изменений, граф проекций (ADR 0010).
- `crates/sat-core/src/entity.rs` — `@sat:entity`: устойчивые id сущностей и реестр id ↔ имя.
- `crates/sat-core/src/identity.rs` — `@sat:identity`: эмодзи и оттенок цепочек и контекстов из состава.
- `crates/sat-core/src/ignore.rs` — `@sat:ignore`: правила игнор-листа конфликтов, нормальная форма, сопоставление.
- `crates/sat-core/src/intent.rs` — `@sat:intent`: намерения проекта (поставить мод из источника), запланированные моды.
- `crates/sat-core/src/ids.rs` — `@sat:ids`: типизированные `u32`-идентификаторы.
- `crates/sat-core/src/metrics.rs` — `@sat:metrics`: каталог метрик мода и их значения.
- `crates/sat-core/src/interner.rs` — `@sat:interner`: арена строк, свёртка регистра, каталоги, `file_key`.
- `crates/sat-core/src/jobs.rs` — `@sat:jobs`: задачи с отменой и поколениями, ворота публикации.
- `crates/sat-core/src/expr.rs` — `@sat:expr`: язык выражений фильтров, правил и счётчиков.
- `crates/sat-core/src/facts.rs` — `@sat:facts`: факты сканера.
- `crates/sat-core/src/table.rs` — `@sat:table`: колонки провайдеров, CSR по путям и модам.
- `crates/sat-core/src/tag_rules.rs` — `@sat:tag-rules`: наборы правил тегов и их применение к фактам мода.
- `crates/sat-core/src/sticker_rules.rs` — `@sat:sticker-rules`: условные стикеры — стикер по выражению над метриками мода.
- `crates/sat-core/src/tags.rs` — `@sat:tags`: каталог тегов и их цвета.
- `crates/sat-core/src/overwrite.rs` — `@sat:overwrite`: проекция перезаписи (параллельно по путям), статистика.
- `crates/sat-core/src/canon.rs` — `@sat:canon`: снимок канона, блоки, приоритеты.
- `crates/sat-core/src/plugin_edit.rs` — `@sat:plugins.edit`: правка канона плагинов и файлы менеджера.
- `crates/sat-core/src/plugins.rs` — `@sat:plugins`: канон плагинов, версии по модам, список плагинов как его видит игра.
- `crates/sat-core/src/routes.rs` — `@sat:routes`: нормализованные цепочки (группы маршрутов точных цепочек).
- `crates/sat-core/src/project.rs` — `@sat:project`: проект, `observe`, `manager_diff`, `book` / `commit_book`.
- `crates/sat-core/src/run.rs` — `@sat:run`: статусы пробега, `Authority`, `seal`.
- `crates/sat-core/src/session.rs` — `@sat:session`: книга порядка + таблица фактов + граф проекций; ленивые `priorities()` / `overwrite()`.
- `crates/sat-core/src/order/mod.rs` — `@sat:order`: оглавление движка порядка (ADR 0008).
- `crates/sat-core/src/order/seq.rs` — `@sat:order.seq`: последовательность кусками, агрегаты, O(log n) запросы.
- `crates/sat-core/src/order/book.rs` — `@sat:order.book`: канон + декорации + вид, операции, патчи, история.
- `crates/sat-core/src/order/cursor.rs` — `@sat:order.cursor`: фокус / якорь / выделение и раскладка клавиш.
- `crates/sat-core/src/order/decor.rs` — `@sat:order.decor`: декорации для проекта, кодек маркеров групп.
- `crates/sat-core/src/order/flow.rs` — `@sat:order.flow`: раскладка потоком — блоки, ряды карточек, окно рядов.
- `crates/sat-core/src/order/tests.rs` — `@sat:order.tests`: модельные тесты.
- `crates/sat-core/src/resource/mod.rs` — `@sat:resource`: манифест, источник, возможности, политика, реестр (ADR 0011).
- `crates/sat-core/src/resource/approvals.rs` — `@sat:resource.approvals`: одобрения пользователя, файл на машине.
- `crates/sat-core/src/resource/command.rs` — `@sat:resource.command`: встроенные команды и их запуск.
- `crates/sat-core/src/resource/hotkeys.rs` — `@sat:resource.hotkeys`: аккорды, раскладка, карта клавиш.
- `crates/sat-core/src/sync/mod.rs` — `@sat:sync`: оглавление синхронизации (ADR 0006).
- `crates/sat-core/src/sync/names.rs` — `@sat:sync.names`: правила имён монолита.
- `crates/sat-core/src/sync/donor.rs` — `@sat:sync.donor`: внешний донор.
- `crates/sat-core/src/sync/records.rs` — `@sat:sync.records`: записи SYNC, сверка.
- `crates/sat-core/src/sync/plan.rs` — `@sat:sync.plan`: план фиксации — изменения канона и упорядоченные, проверенные операции на диске.

### `crates/sat-app` — API приложения (ADR 0004)

- `crates/sat-app/src/lib.rs` — `@sat:app`: `Command` / `Query` / `Reply` / `Event`, `App` — диспетчер над сессией и реестром ресурсов; строки окна со счётчиками и бейджами карточки, окно рядов потока (`@sat:app.lines`), сводка мода, редактор декорации (`@sat:app.decoration`, `@sat:app.patch-style`).
- `crates/sat-app/src/chains.rs` — `@sat:app.chains`: заголовки нормализованных цепочек (значок, имя, «решено»).
- `crates/sat-app/src/context.rs` — `@sat:app.context-table`: таблица конфликтов контекста, окно строк × столбцов.
- `crates/sat-app/src/sync.rs` — `@sat:app.sync`: записи SYNC в приложении — решения командой, план запросом.
- `crates/sat-app/src/sheet.rs` — `@sat:app.sheet`: карточка мода — схема, уровни, слои наблюдённого и пользователя.
- `crates/sat-app/src/find.rs` — `@sat:app.find`: вид мода для выражений, поиск модов по условию, стикеры правил.

### `apps/devserver` — оболочка разработки (ADR 0004)

- `apps/devserver/src/main.rs` — `@sat:devserver`: загрузка сборки MO2 только на чтение (`@sat:devserver.load`), HTTP на `127.0.0.1` (`@sat:devserver.http`: `POST /command`, `POST /query`, `GET /`). Страница — `apps/devserver/src/index.html` (истины не держит): поток карточек по окну рядов, контекстное меню и палитра, сводка («Мод», «Декорация мода», «Конфликты» с участниками), таблица контекста на canvas, плагины.

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
- `crates/sat-mo2/src/commit.rs` — `@sat:mo2.commit`: исполнение плана — версия-бэкап, журнал, операции, запись профиля, откат.
- `crates/sat-formats/src/fomod.rs` — `@sat:formats.fomod`: установщики FOMOD — чтение `ModuleConfig.xml`, умолчания, выбор → файлы.
- `crates/sat-mo2/src/install.rs` — `@sat:mo2.install`: установка мода из архива — корень Data, факты до установки, безопасная распаковка.
- `crates/sat-mo2/src/profile.rs` — `@sat:mo2.profile`: файлы профиля байт в байт.
- `crates/sat-mo2/src/meta.rs` — `@sat:mo2.meta`: `meta.ini`.

### `crates/sat-stll` — проект на диске (ADR 0007)
- `crates/sat-stll/src/lib.rs` — `@sat:stll`: проект на диске; открытие папки и перевод ранней раскладки.
- `crates/sat-stll/src/file.rs` — `@sat:stll.file`: файл `.stll` — манифест, разделы, факты, атомарное сохранение.
- `crates/sat-stll/src/sections.rs` — `@sat:stll.sections`: дерево проекта ↔ разделы NDJSON.
- `crates/sat-stll/src/facts.rs` — `@sat:stll.facts`: формат факта мода; ранняя раскладка-папка.
- `crates/sat-stll/examples/save_bench.rs` — `@sat:stll.bench`: время сохранения и чтения проекта в масштабе сборки.

### `crates/sat-scan` — обход папок модов (холодный ярус)
- `crates/sat-scan/src/lib.rs` — `@sat:scan`: `ScanJob`, `ScanOptions`, `ScanProgress`, `scan` / `scan_batched`, выбор маршрута.
- `crates/sat-scan/src/analyzer.rs` — `@sat:scan.analyzer`: контракт, реестр, встроенные анализаторы, `plan`.
- `crates/sat-scan/src/walk.rs` — `@sat:scan.walk`: обход мода, `reanalyze`.
- `crates/sat-scan/src/signature.rs` — `@sat:scan.signature`: сигнатуры v3 / v2 / v1.
- `crates/sat-scan/src/cache.rs` — `@sat:scan.cache`: формат 2 со штампами, чтение формата 1.
- `crates/sat-scan/tests/scan.rs` — `@sat:scan.it`: интеграционный тест.

### `tools`
- `tools/sat-cli/src/main.rs` — `@sat:cli`: `analyze`, `donor`, `sync`, `commit`, `ignore`, `inspect`, `bench-synth`.
- `tools/sat-cli/src/commit_cmd.rs` — `@sat:cli.commit`: `sync` (записи и решения) и `commit` (план, `--apply`).
- `tools/sat-cli/src/ignore_cmd.rs` — `@sat:cli.ignore`: игнор-лист конфликтов в проекте.
- `tools/sat-cli/src/intent_cmd.rs` — `@sat:cli.intent`: `plan-install` — запланированный мод из архива загрузок.
- `tools/sat-cli/src/stickers_cmd.rs` — `@sat:cli.stickers`: условные стикеры проекта — список, добавить, убрать.
- `tools/sat-cli/src/project_io.rs` — `@sat:cli.project`: файл проекта сборки для команд — открыть, перевести раз, сохранить.
- `tools/sat-cli/src/bench_order.rs` — `@sat:cli.bench-order`: бенчмарк движка порядка.

## Документы
- `docs/CHEATSHEET.md` — шпаргалка владельца: как взять `sat-cli`, команды, как читать вывод, что сверить с MO2. Меняешь флаги или вывод CLI — обнови её в том же коммите.
- `PHILOSOPHY.md` — направление и правила. `docs/adr/` — решения (0010 — ядро и движок, 0011 — реестр ресурсов, 0012 — стиль карточки и раскладка потоком). `docs/distances/` — текущая работа.
- `docs/parity/` — что делает монолит (эталон поведения). `docs/semantic/monolith/` — конспект монолита (`12-mod-card.md` — карточка мода, бейджи, декорация, таблица конфликтов):
  `00-index.md` (области и где они), `GOLD.md` (идеи, которые переносим), `structure.md` (генерирует `tools/monolith-map.py`).
- `docs/semantic/old-satellite/` — конспект прототипа «Satellite V2 Constructor Core»: `00-conspectus.md` (по слоям; §13–14 — построчное дочитывание пробега и остальных областей,
  с пометками берём / меняем / ошибка), `BORROW.md` (что переносим и что не повторяем), `structure.md` (разделы).
- `tools/check-map.sh` → `tools/check-map.py` — проверка этой карты и якорей в CI. `tools/monolith-map.py` — структурная карта монолита.
