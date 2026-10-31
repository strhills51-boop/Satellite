# 11 · SYNC — сводный конспект области (проход 2)

> Строки — монолит `index.html` (196 155 строк). «IIFE» — главное замыкание приложения: `(() => {` 19 456 … `})();` 166 057.
> Источники: `pass/S02` (модель, сверка), `pass/S03` (план, модалка, исполнение), `pass/S21`/`S22` (CP5, v526Raw, гонтлет),
> `pass/S12` (структурные мутации), `pass/S06` (autosave), `pass/S11`/`S12` (донор), `05-run.md` п. 0, ADR 0006.
> Пометки: **[код П2]** — перепроверено по коду во втором проходе; **[S0n]** — со слов прохода 1, код не перечитывался.

## 1. Коротко

- SYNC — слой расхождений «проект ↔ папка `mods` ↔ профиль MO2». Расхождения становятся **записями** (9 типов) со статусом
  `pending / confirmed / ignored / resolved`. Записи видны прокси-модами в служебном разделителе «SYNC / Синхронизация»
  (всегда первый, выключен, в `modlist.txt` не пишется).
- Записи приходят из двух мест: (а) сверка диска с проектом в каждом пробеге (`buildSyncReconcileResult` 35 082) —
  `fs_created / fs_deleted / fs_rename_candidate`; (б) правки пользователя — `project_created / renamed / deleted /
  restored / modlist_reordered`, плюс `external_migration_pending` от донора.
- Фиксация: модалка из 5 шагов → `buildSyncCommitPlan` 35 520 (12 видов операций, 5 стадий) → pre-commit
  (`runSyncPrecommitCheck` 35 938, surface-пробег + отпечаток записей) → `executeSyncCommitPlan` 37 178: бэкап в
  `profile/monolith/versions/<ts>/`, удалённое — в `profile/monolith/deleted/<ts>/`, новый `modlist.txt` и `project.json`
  в корне профиля, манифест `commit.rollback.json` (только опись, автоматического отката нет).
- **Живой путь — лексические функции IIFE.** Обёртки Run Kernel CP5 (`window.*`, 175 643–175 649) из UI недостижимы (§2.0).
  Весь v526Raw (граф эффектов, гонтлет) — dry-run и самотесты; физическую границу он только сравнивает по идентичности.
- Главные дефекты (§5): каждая сверка **сбрасывает решения** по FS-записям в `pending` (FS-фиксация фактически
  заблокирована); операции плана идут в порядке «новые записи первыми» без зависимостей, а слияние записей по отпечатку
  теряет исходное имя (цепочки «создал → переименовал», «переименовал дважды» ломают диск); записи и история чистятся
  **до** записи файлов; перенос = копия всего дерева + удаление.

## 2. Живой путь

### 2.0. Какая реализация работает (ключевой вопрос) — **решено [код П2]**

Ответ: **кнопки SYNC-модалки исполняют лексические оригиналы из IIFE** (`buildSyncCommitPlan` 35 520,
`runSyncPrecommitCheck` 35 938, `executeSyncCommitPlan` 37 178). Обёртки CP5 (`window.* = …AuthoritativeCP5`,
175 643–175 649) из UI **не вызываются никем**. Доказательства:

1. 19 456 — `(() => {`, 166 057 — `})();` (проверено чтением). Все три функции объявлены внутри как
   `function …` (отступ 6; xref `depth=1`) — это привязки области стрелочной функции, а не свойства `window`.
2. Внутри IIFE этих имён никто не переприсваивает (`executeSyncCommitPlan =` встречается только как
   `window.executeSyncCommitPlan =` 175 649). Голые вызовы 36 342/36 439 (`buildSyncCommitPlan()`), 36 424
   (`runSyncPrecommitCheck(backdrop, logLines)`), 36 463 (`executeSyncCommitPlan(currentPlan)`), гонтлет 37 515/37 522
   и параметры по умолчанию (36 017, 36 276, 36 300, 37 178) резолвятся в лексические привязки.
3. Обработчики модалки — `addEventListener` внутри `renderSyncModalBody` (36 402–36 483), то есть замыкания IIFE.
   Inline-`onclick` для SYNC нет. Открытие — `data-sync-action="open"` + `addEventListener` (35 476, 104 671), меню —
   `action: () => openSyncModal()` (79 946, 80 081, 108 702, 108 745).
4. Мост `Object.assign(window, {…})` 134 686–134 694 публикует оригиналы под **другими** именами:
   `legacyBuildSyncReconcileResultCP8`, `legacyBuildSyncCommitPlanCP8`, `legacyRunSyncPrecommitCheckCP7`,
   `legacyExecuteSyncCommitPlanCP7` (и `analyzeModsDirectory`, донорские). Имён `window.buildSyncCommitPlan` /
   `runSyncPrecommitCheck` / `executeSyncCommitPlan` до 175 643 нет: CP5 их **создаёт**, а не подменяет.
5. Кто зовёт `window.*`-версии:
   - `window.executeSyncCommitPlan(...)` и `window.runSyncPrecommitCheck(...)` **не вызывает никто**. Все ~60 ссылок
     в 178 216–190 945 — сравнение идентичности (`PHYSICAL_BOUNDARY_AT_BOOTSTRAP = window.executeSyncCommitPlan` …
     `check('physical-boundary-unchanged', window.executeSyncCommitPlan === …)`).
   - `window.buildSyncCommitPlan()` **вызывается** v526Raw (178 375, 178 545, 179 275, 179 350, 180 103, 180 210,
     182 490–182 648) — прозрачная обёртка `domainPlan.apply` (тот же расчёт, с той же побочной записью в `sync.commit`).
   - `ModlistToolSyncIssuanceV503.{preflight, issue}` и `ModlistToolSyncPhasesV503.applySyncPlanThroughExistingBoundary`
     из кода не вызываются; единственный потребитель — геттер `sync: () => …` в интеграционном инспекторе 177 250 (чтение).
6. `LEGACY` 165 505–165 525 (внутри IIFE) — снимок ссылок на лексические функции для описания, не вызов.

Следствия:
- Физическая фиксация в проде — `executeSyncCommitPlan` 37 178 **напрямую**. Run Kernel / Run Store её не видит,
  барьеры CP5 (`TARGET_REVISION`, `USER_APPROVAL`) не работают; защищают только ворота модалки (§4.4).
- Pre-commit из модалки зовёт лексический `analyzeModsDirectory('surface')` 134 655 (cutover) — surface-пробег
  **мимо** CP4 intake (`05-run.md` п. 0б).
- Ошибочны: `pass/S21` 175 465 («все внутренние вызовы идут через ядро»), пометки «Статус: перекрыто» в `pass/S03` §2
  (35 520, 35 938) и «после 175 649 это CP5-обёртка» (36 463). Верна таблица `pass/S03` §3.1.
- CP5 issuance и фазы достижимы только из консоли или внешнего кода. Для NG это описание намерения, не поведение.

### 2.1. Сквозной путь: от правки до диска

1. **Запись появляется.**
   - Пробег (любой режим) → `readModsEntriesFromHandle` → `buildSyncReconcileResult(folderNames, {})` 134 083
     (cutover, **пустой** `surfaceResult`, [код П2]) → `fs_created` / `fs_deleted` (+ `fsMissing` на сущности).
     `fs_rename_candidate` в живом пути **не рождается никогда**: подписи берутся из `surfaceResult.nextSignatures`,
     а он пуст. Вызов с настоящим surface-результатом (132 640) — в legacy-пути, который не живой.
   - Правки пользователя (§4.2): `createNewSeparator`, `addEmptyMod`, `renameSeparator` / `renameMod` / массовое
     переименование 79 546, `removeSeparator`, `deleteModsToRecycle` (Delete), перестановки (`upsertModlistOrderSyncRecord`),
     восстановление припаркованного (`restoreSyncParkedEntity`, `prepareMovedSyncRealModForProject`), советник порядка
     145 309, донор (`createExternalMigrationRecords` 107 822, лексический вызов 107 695).
   - Каждая запись проходит `upsertSyncRecord` 34 883: слияние по `fingerprint`, пересчёт `commit.pendingCount/blocked`,
     прокси в SYNC-разделителе (кроме исключений §4.1).
2. **Решение пользователя.** Кнопки на карточках и прокси (`data-sync-status-id/value`, `data-sync-resolution-id/value`;
   делегированные клики на доске [S11, обл. 21] и модалка 36 472–36 483) → `setSyncRecordStatus` 36 165 / `setSyncRecordResolution` 36 187;
   «подтвердить все проектные» — `confirmProjectSideSyncRecords` 36 211. Восстановление — `data-sync-restore-mod`.
3. **Модалка** `openSyncModal` 36 486 → `#syncModalBackdrop`, `renderSyncModalBody` 36 337 (план строится на **каждый**
   рендер). 5 шагов (`getSyncFlowState` 36 276):
   1) mods folder (`pickSyncModsFolder` 36 499); 2) profile folder (`pickSyncProfileFolder` 36 514);
   3) pre-commit check (кнопка → `runSyncPrecommitCheck`); 4) refresh versions (`refreshSyncExternalVersionsIndex`
   37 045, читает `monolith/versions/*/commit.rollback.json`, `NotFoundError` = «первый commit», это не ошибка);
   5) execute (кнопка 36 438 → свежий план → `summarizeSyncPrecommitReadiness` → `plan.blocked` → handles → `confirm()`
   → `executeSyncCommitPlan(currentPlan)`). Чекбокс `#syncCommitPluginsToggle` → `sync.commit.plugins`.
   Шаг 3 в индикаторе «мягкий» (`status==='ok'` и имя mods), но кнопка 5 требует **свежести** (§4.5).
4. **Исполнение** 37 178 (§4.6) → артефакты, `rememberSyncCommitSession`, обновление версий, `autosave('sync-commit')`
   (создаёт базу autosave), `renderAllPreserveScroll()`.

### 2.2. Что вокруг, но не в живом пути

- **Run Kernel CP5** 175 465–175 702 (`createRunKernelRecoveryCP5AuthoritativeSyncIssuanceRuntimeOwnerV527`):
  маршрут preflight `collect-intent → obtain-analysis-evidence → reconcile (= legacy precommit) → compile-desired-changes
  (= legacy plan, фиксирует targetRevision {profile, versionsLoadedAt, projectUpdatedAt}) → preflight-barriers`
  (USER_APPROVAL всегда `pass:false` ⇒ пробег всегда `blocked`, holder в `pending`); маршрут apply
  `commit-barriers (DEPENDENCIES_READY, TARGET_REVISION, USER_APPROVAL) → stage (no-op) → existing-SYNC-commit
  (= legacy execute) → verify (всегда ok) → promote`. `window.executeSyncCommitPlan` передаёт `userApproved:true`,
  так что одобрение формальное. Фазы 175 668–175 702 — алиасы (`evidence/plan/apply`). Мёртвый для UI код (§2.0).
- **v526Raw** (S21/S22): CP3 «паритет графа эффектов» 179 428 переводит legacy-план в эффекты (dry-run) и явно
  перечисляет **неявные действия исполнителя** (§4.6, п. 9); гонтлет CP1 — каталог 46 действий UI → ожидаемые типы
  записей → операции, с честными `contract-gap`; CP2 — оракул «действие → записи → план» (виртуальный двойник
  `buildLegacyPlan` 181 200); CP3 — физический commit в песочнице через приватный мост
  `ModlistToolSyncGauntletPhysicalBridgeV526Raw` 37 564 (лексический execute, подменённый `state`, подавленный autosave);
  release 183 013 — 10 записей, идемпотентный повтор и коллизия rename. Всё запускается вручную.
- **Панель развёртывания** в SYNC — область 13 (v527 Deployment), здесь не разбирается.

## 3. Модель данных

### 3.1. `project.sync` (`createEmptySyncState` 34 599, `SYNC_LAYER_VERSION = 2`)
```
{ version: 2, separatorId, records[], resolvedHistory[≤200], lastReconcileAt,
  analysisBasis { surfaceRunAt, reconcileAt, modsPath, signaturesCount },
  commit { lastPlanAt, lastCommitAt, blocked: true, pendingCount, plugins: false,
           + lastSession{id,timestamp,status,startedAt,finishedAt,applied,warnings,errors}, lastCommitId,
             lastVersionFolder, history: [] },
  precommitCheck { status: idle|running|ok|blocked|failed, checkedAt, modsName, surfaceRunAt, reconcileAt,
           signaturesCount, changedCount, activeRecords, pendingCount, confirmedCount, blockerCount,
           warningCount, fingerprint, message } }
```
Вне проекта: `state.syncVersions {loading, loadedAt, error, profileName, items[], firstCommit?}`,
`state.syncCommitExecuting`, `state.fs.{modsHandle, profileHandle}`.

### 3.2. Запись (`normalizeSyncRecord` 34 638) [код П2]
`{id, fingerprint, entityType: mod|separator|modlist, recordType, source: project|fs|reconcile|external,
status (по умолчанию 'pending'), entityId, oldName, newName, fsName, projectName, confidence: manual|exact|signature|contains,
signature, details, resolution, resolutionStatus, separatorId, orderStamp, proxyModId, donorSessionId, donorName,
sourceOrder, sourceEnabled?, sourceSeparatorName, hasFolder?, hasModlistEntry?, migrationStatus, createdAt, updatedAt}`.
- `source` по умолчанию: `recordType` начинается с `fs_` → `'fs'`, иначе `'project'`. «FS-запись» в смысле кода —
  `source !== 'project'` (`isFsSyncRecord` 34 822), то есть `fs`, `reconcile` **и `external`**.
- Отпечаток по умолчанию `[recordType, entityType, entityId||oldName||fsName, newName].join('::')`; почти все
  создатели задают свой (§4.2). Лишние поля (например `externalCandidateId`) нормализация отбрасывает.
- Типы (9): `project_created, project_deleted, project_renamed, project_modlist_reordered, project_restored,
  fs_created, fs_deleted, fs_rename_candidate, external_migration_pending`.
- Решения FS (`getSyncFsResolutionOptions` 34 796): `fs_created` → `adopt_to_project | move_to_deleted | acknowledge`;
  `fs_deleted` → `create_disk_folder | remove_project_entity | acknowledge`; `fs_rename_candidate` → `accept_fs_name |
  rename_disk_to_project | acknowledge`; прочие не-проектные (в том числе `external_migration_pending`) → `acknowledge`.
  Выбор решения ставит `status='confirmed'`, `resolutionStatus='selected'`.

### 3.3. Моды в SYNC-разделителе (`mod.sync`, `normalizeModSync` 34 700)
`{role, recordId, recordType, entityType, status, oldName, newName, fsName, source, originalSeparatorId, originalEnabled}`.
Роли: `sync_proxy` (`isSyncProxy`; имя `[FS|PROJECT <ТИП>] <title>`, выключен, стикер ⚠️ для `source==='fs'`, иначе 📌;
встаёт в начало), `parked_real_mod` (настоящий мод, удалённый в проекте), `separator_representative_snapshot`
(представитель удалённого разделителя `<имя>_separator`, стикер 💀). Плюс вне SYNC: `external_migration_placeholder`
(черновик донора в целевом разделителе) и `sep.sync/mod.sync.fsMissing` у реальных сущностей.
`ensureSyncStructure` 34 826 при каждом чтении: находит или создаёт разделитель, ставит его первым, `enabled=false`,
каноническое имя, перенормализует `sep.mods.map(normalizeMod)`.

### 3.4. План (`buildSyncCommitPlan` 35 520) [код П2]
`{version:1, timestamp (ISO с '-' вместо ':' и '.', '_' вместо 'T', без 'Z'), blocked, pendingCount, confirmedCount,
profileName, modsName, paths{monolith, version, deleted = profiles/<p>/monolith{,/versions/<ts>,/deleted/<ts>}, mods},
records[confirmed: id, entityType, recordType, source, status, old/new/fs/projectName, confidence, resolution],
ops[], warnings[], commitPlugins, diagnostics, validationBlocked}`.
Стадии: `bootstrap` (3× `mkdir`) → `backup` (`backup_file` modlist [+plugins, loadorder], `write_file` project.before.json)
→ `apply | apply_project | ack` → `finalize` (`write_file` modlist.txt [+plugins.txt, loadorder.txt], project.json).
12 видов операций: `mkdir, backup_file, write_file, rewrite_modlist_order, move_to_deleted, create_mod_folder,
create_separator_folder, rename_folder, fs_adopt_to_project, fs_remove_project_entity, fs_rename_project_entity,
acknowledge_fs_record`. Исполнитель смотрит только на стадии `apply|apply_project|ack`; bootstrap, backup и finalize
он выполняет **своим** кодом, а не по ops (ops этих стадий — описание).

### 3.5. Сессия фиксации (`createSyncCommitSession` 36 875)
`{id: uid('commit'), timestamp, startedAt, finishedAt, status: running|completed|completed_with_warnings|
completed_with_errors|failed, profileName, modsName, log[], warnings[], errors[], appliedRecordIds[], skippedRecordIds[],
rollback{movedToDeleted[{from,to,recordId}], renamedFolders[], createdFolders[{path,recordId,type}],
writtenFiles[{path,type}], backups[]}}`. `completed_with_errors` недостижим: ошибки пишет только `markFailed`.

## 4. Точные правила

### 4.1. `upsertSyncRecord` 34 883 [код П2]
1. `record = normalizeSyncRecord(raw)`; совпадение — первая запись с тем же `fingerprint` **или** `id`.
2. Совпала → `Object.assign(existing, {...existing, ...record, id: existing.id, createdAt: existing.createdAt, updatedAt: now})`.
   Поскольку `record` нормализован, **все** поля берутся из нового вызова: `status` (по умолчанию `pending`),
   `resolution` (`''`), `oldName`, `proxyModId` (`''`) и т. д. Не совпала → `records.unshift(record)` (новые — первыми).
3. `commit.pendingCount = #pending`, `commit.blocked = pendingCount > 0`.
4. `createProxy` (по умолчанию true) → `ensureSyncProxyForRecord`: не для `ignored`, не для `project_deleted` мода,
   не для `project_deleted` разделителя в режиме `snapshot`. Прокси ищется по `proxyModId`, затем по `sync.recordId`;
   найден — обновляется имя и `mod.sync`, иначе создаётся в начале SYNC-разделителя.

### 4.2. Кто создаёт записи (отпечатки)
| Действие | Запись | Отпечаток | Прокси / роль |
|---|---|---|---|
| Сверка: папка без сущности | `fs_created`, source fs, conf exact | `fs_created::<separator\|mod>::<norm(fsName)>` (разделитель — суффикс `_separator`, без учёта регистра) | прокси ⚠️ |
| Сверка: сущность без папки | `fs_deleted`, `oldName=projectName` | `fs_deleted::<type>::<entityId\|norm>` + `fsMissing=true` | прокси ⚠️ |
| Сверка: подпись совпала ровно с одной папкой | `fs_rename_candidate`, source reconcile | `fs_rename_candidate::<type>::<entityId>::<norm(fsName)>` | прокси 📌 (source≠'fs') |
| `createNewSeparator` 110 266, советник 145 309, 79 365 | `project_created` separator, `newName=<имя>_separator` | `project_created::separator::<id>` | прокси 📌 |
| `addEmptyMod` 110 318 | `project_created` mod, `newName='New Mod'` | `project_created::mod::<id>` | прокси |
| `renameSeparator` 110 340 | `project_renamed`, `<old>_separator → <new>_separator` | `project_renamed::separator::<id>` | прокси |
| `renameMod` 110 365, массово 79 546 | `project_renamed` (у припаркованного — только `mod.sync.newName`, `oldName` хранит первое) | `project_renamed::mod::<id>` | прокси |
| `removeSeparator` 110 486 | `project_deleted` separator, `oldName=<имя>_separator`, моды уходят соседу | `project_deleted::separator::<id>` | snapshot 💀 |
| `deleteModsToRecycle` 112 416 (Delete) | `project_deleted` mod, `oldName=mod.name`; мод паркуется в SYNC | `project_deleted::mod::<id>` | `parked_real_mod`, без прокси |
| Перестановки | `project_modlist_reordered`, entityType modlist, `entityId='modlist.txt'` | `project_modlist_reordered::modlist.txt` (одна на проект) | прокси |
| Восстановление припаркованного | старая запись → `resolvedHistory` (`removeSyncRecordAndCards` 112 299); новая `project_restored` **pending** | `project_restored::mod::<id>` | без прокси |
| Донор `createExternalMigrationRecords` 107 822 | `external_migration_pending`, source external, `fsName`=папка донора, `separatorId` цели | `external_migration_pending::<session>::<key>::<targetSep>` | без прокси; черновик-мод в цели |

Без записи: вкл/выкл мода (идёт через modlist при фиксации), быстрые перестановки «conflict-chain-order» /
«phone-interactive-order» и `reorderSeparatorsCanonicalCP19` (телефон) [S12] — см. §7.

### 4.3. Сверка `buildSyncReconcileResult(folderNames, surfaceResult)` 35 082 [код П2]
Сравнение по `normalizeName` (= `sat-core::sync::names`, сверено).
1. Сущности проекта (`getProjectSyncComparableEntities` 35 015): все разделители, кроме SYNC и системных, под именем
   `<name>_separator` (подпись пустая); моды, кроме представителей и SYNC-прокси (**припаркованные, snapshot-моды
   разделителей, черновики донора входят**), подпись `mod.analysisRef.surfaceSignature`.
2. Плановые множества из проектных записей не в `ignored|resolved` (`getPlannedSyncMismatchSets` 35 045):
   `diskExtraNames` ← `project_deleted` (`getSyncRecordPathName` = old‖new‖fs‖project) и `project_renamed.oldName`;
   `projectMissingNames` ← `project_created.newName` (+`_separator`) и `project_renamed.newName`.
3. Папка не сопоставлена и не в `diskExtraNames` → `fs_created`.
4. Сущность не сопоставлена и не в `projectMissingNames` → `fs_deleted`, `fsMissing=true`; сопоставленной →
   `fsMissing=false`.
5. Переименование: несопоставленные папки раскладываются по подписи; сущность, чья подпись совпала **ровно с одной**
   папкой → `fs_rename_candidate`. Пара `fs_created` + `fs_deleted` при этом **остаётся** (три записи на одно событие).
6. `lastReconcileAt = now`, `analysisBasis.{reconcileAt, signaturesCount}`, пересчёт `commit.blocked`.
7. Записи, чьё расхождение исчезло, **не закрываются**: сверка только добавляет и обновляет.

### 4.4. Запись → операция (план 35 573–35 748) [код П2]
В план идут только `confirmed`; `pending` блокирует; `ignored|resolved` пропускаются. Проектные (`source==='project'`)
идут первыми, затем все прочие; внутри — в порядке `sync.records` (**новые первыми**).
| source | recordType | resolution | op | stage |
|---|---|---|---|---|
| project | `project_modlist_reordered` | — | `rewrite_modlist_order` (no-op; порядок пишет finalize) | ack |
| project | `project_deleted` | — | `move_to_deleted` `mods/<path>` → `deleted/<ts>/<path>` | apply |
| project | `project_created` | — | `create_separator_folder` (`<n>_separator` + meta.ini) / `create_mod_folder` (`newName‖fsName`) | apply |
| project | `project_renamed` | — | `rename_folder` old → new | apply |
| project | `project_restored`, прочие | — | **нет операции** (запись не применяется и не чистится) | — |
| не project | `fs_created` | `adopt_to_project` | `fs_adopt_to_project` | apply_project |
| не project | `fs_created` | `move_to_deleted` | `move_to_deleted` | apply |
| не project | `fs_deleted` | `create_disk_folder` | `create_*_folder` из `oldName‖projectName‖newName` | apply |
| не project | `fs_deleted` | `remove_project_entity` | `fs_remove_project_entity` | apply_project |
| не project | `fs_rename_candidate` | `accept_fs_name` | `fs_rename_project_entity` | apply_project |
| не project | `fs_rename_candidate` | `rename_disk_to_project` | `rename_folder` fs → project | apply |
| не project | иначе (`acknowledge`, `''`, `external_migration_pending`) | | `acknowledge_fs_record` | ack |

Диагностика плана `buildSyncDiagnostics` 36 017 (блокеры делают `blocked=true, validationBlocked=true`):
блокеры — `no_active_sync_records`, `versions_refresh_required` (нет `syncVersions.loadedAt` для этого профиля),
`fs_resolution_missing` (confirmed не-проектная без resolution), `duplicate_sync_separators`, `duplicate_target_ops`
(несколько create/rename в одну цель, ключ `toName‖folderName` в нижнем регистре), `empty_rename_name`,
`empty_create_name`, `empty_deleted_name`, `remove_non_empty_separator`, `sync_export_leak` (имя SYNC в
`serializeModlist()`), `sync_proxy_export_leak` (имя прокси или припаркованного мода — **подстрока** modlist).
Предупреждения: `duplicate_records`, `same_rename_name`, `create_name_exists_in_project`, `modlist_export_check_failed`.
Побочный эффект построителя: `sync.commit.{lastPlanAt, pendingCount, blocked}` — на каждый рендер модалки.
Мёртвые: `buildSyncPreCommitGuard` 35 782, `renderSyncPrecommitStatusHtml` 35 912 [S03].

### 4.5. Pre-commit и свежесть [код П2]
- `runSyncPrecommitCheck` 35 938: нужен `modsHandle`; `precommitCheck.status='running'` → `startRunSession('Pre-commit
  sync check', …, {deepEnabled:false, archiveScanEnabled:false, tagRulesEnabled:false, stickerRulesEnabled:false,
  customArmed:false})` → `await analyzeModsDirectory('surface')` (лексический cutover; внутри **сверка**, §5 п. 1).
  Провал, если `runs.surface.lastRunAt` не сменился. Иначе: блокеры = confirmed не-проектные без resolution,
  предупреждения = `surface.changedSignatures.length` (только счёт, не блок). Пишет полный `precommitCheck`
  (status `ok|blocked`) и `autosave('sync-precommit-check')`.
- Отпечаток `getSyncRecordsFingerprint` 35 849: по активным записям
  `[id, recordType, entityType, source, resolution, oldName, newName, fsName, projectName, fingerprint].join(':')`,
  сортировка, `join('|')`. **Без** `status` и `updatedAt` (статус — дело ворот, не свежести).
- Свежесть `isSyncPrecommitCheckFresh` 35 876: `status==='ok'` ∧ `checkedAt` ∧ `normalizeName(modsName)` совпадает с
  выбранной папкой ∧ `surfaceRunAt === runs.surface.lastRunAt` ∧ `reconcileAt === sync.lastReconcileAt` ∧ отпечаток совпал.
- Ворота кнопки execute (`summarizeSyncPrecommitReadiness` 35 889 + 36 445–36 458): есть активные записи; выбраны mods
  и profile; pre-commit свеж; `pendingCount=0`; блокеров диагностики нет; `plan.blocked=false`; `confirm()`.
  Шаг 4 (versions) — через блокер `versions_refresh_required`.

### 4.6. `executeSyncCommitPlan(plan)` 37 178 — точный порядок [код П2]
0. `plan.blocked` → throw; нет `modsHandle`/`profileHandle` → throw. Сессия.
1. `profileHandle/monolith/`, `…/versions/`, `…/deleted/`, `…/versions/<ts>/`, `…/deleted/<ts>/` (создаются всегда, даже
   если удалять нечего). Пути в плане `profiles/<p>/…` совпадают с этим, только если `profileHandle` — папка профиля.
2. Бэкап `modlist.txt` в `versions/<ts>/` (любая ошибка чтения → заглушка `# modlist.txt was not found before commit`);
   при `commitPlugins` — `plugins.txt`, `loadorder.txt` так же.
3. `versions/<ts>/project.before.json` = `exportProjectObject('full')` (при ошибке — `'autosave'`).
4. Цикл по ops (только `apply|apply_project|ack`; после `failed` — выход), каждая op в try/catch → `markFailed`:
   - `fs_adopt_to_project`: разделитель → новый `<имя без _separator>` (цвет `#b45309`) перед SYNC; мод → в разделитель
     «FS / Imported from сборки» (создаётся перед SYNC), `enabled:true`, стикер 🆕; уже есть — skip, но «применено».
   - `fs_remove_project_entity`: разделитель с реальными модами → `false` (не применено); иначе удалить +
     `compactAnalysisMetricSummaryEntitiesV634`; мод → удалить из всех не-SYNC + `retireAnalysisMetricSummaryEntitiesV634`.
     Не найдено → warn, «применено».
   - `fs_rename_project_entity`: `sep.name = stripSeparatorSuffix(toName)` / `mod.name = toName`; не найдено → «применено».
   - `move_to_deleted`: нет источника → warn, **«применено»**; иначе `getUniqueDirectoryName(deleted/<ts>, name)`
     (`name__2`, `__3` …) → `moveDirectoryFromParent` (= рекурсивная копия + `removeEntry(recursive)`).
   - `create_mod_folder`: пустое имя → skipped; `getDirectoryHandle(create)`; существовала — ok.
   - `create_separator_folder`: то же + **перезапись** `meta.ini` шаблоном `SYNC_SEPARATOR_META_INI` 35 484
     (`modid=0`, `gameName=SkyrimSE`, фиксированный цвет, `nexusLastModified=2025-02-12T16:56:35Z`), старый не бэкапится.
   - `rename_folder` (`renameDirectoryInParent` 36 612): `skip` (пустое или то же имя) → warn, «применено»;
     `missing` → warn, «применено»; `target_exists` → `markFailed` (стоп цикла); `renamed` = копия + удаление.
   - `rewrite_modlist_order`, `acknowledge_fs_record` → лог, «применено».
5. Успех (`!failed`), в таком порядке: `cleanupCommittedSyncRecords(applied)` (удалить записи, `resolvedHistory=[]`,
   убрать из SYNC прокси/припаркованных/snapshot этих записей) → `status = completed|completed_with_warnings` →
   `resetProjectRuntimeHistoryAfterCommit` (`journal=[]`, undo-стек `historySnapshots=[]`, `historyCursor=-1`,
   `meta.{dirty:false, lastCommittedAt, lastCommitId, lastCommitVersion, versionHistorySource}`,
   `sync.commit.{lastCommitAt, lastCommitId, lastVersionFolder, history:[]}`) → `rememberSyncCommitSession` →
   `serializeModlist()` и финальный JSON → **запись** `profile/modlist.txt` → [`plugins.txt`, `loadorder.txt` из
   `buildPluginCommitFiles`] → `profile/project.json` (полный экспорт с вычищенными записями, пустыми journal,
   resolvedHistory, commit.history).
6. Провал: лог «commit stopped before final modlist/project.json/cleanup». **Уже сделанные** переносы, переименования,
   создания и изменения проекта (`apply_project`) не откатываются.
7. `finally` (всегда): `versions/<ts>/commit.rollback.json` (манифест: `version:1, commitId, timestamp, status, …,
   backupModlist, projectBefore, projectFinal:'project.json', commitReport, appliedRecordIds, skippedRecordIds, rollback,
   warnings, errors, note` «манифест, не автоматическое восстановление») и `commit.report.txt` →
   `rememberSyncCommitSession` → `refreshSyncExternalVersionsIndex({force})` → `autosave('sync-commit'|'sync-commit-failed')`
   → `renderAllPreserveScroll()`. Возвращает `session.log`.
8. Раскладка на диске после успеха (сверено с оракулом гонтлета CP3, S22 181 740):
   `profiles/<P>/modlist.txt`, `[plugins.txt, loadorder.txt]`, `project.json`;
   `profiles/<P>/monolith/versions/<ts>/{modlist.txt, [plugins.txt, loadorder.txt], project.before.json,
   commit.rollback.json, commit.report.txt}`; `profiles/<P>/monolith/deleted/<ts>/<папки>`.
9. Неявные действия, которых нет в ops (так их перечисляет граф эффектов CP3 179 383): до finalize — cleanup записей,
   сброс истории, `rememberSyncCommitSession`; в `finally` — артефакты, `rememberSyncCommitSession`, обновление версий,
   `autosave`, полный рендер.

### 4.7. Сериализация modlist при фиксации (`serializeModlist`, owner 70 165) [код П2]
Разделители с конца к началу, моды с конца к началу; пропуск SYNC-разделителя, прокси и припаркованных; представитель
разделителя → `±<name>_separator` по `sep.enabled`; мод → `±name` по `mod.enabled`; в конце `exportState.bottomLines`;
`\n`. Системные разделители (Skyrim, Overwrite, PLUGINS) и черновики донора **не фильтруются** (§7). Обёртки логических
групп (52 687–52 747) — область 3.

### 4.8. SYNC и autosave [S06, код П2 для 66 445]
`sync` целиком входит в лёгкое состояние autosave и в `structure_state`. Причины `sync-commit`, `sync-commit-failed`
создают базу autosave. Локальная компактная база (`stripProjectForLocalAutosave` 66 348; compact — 1 200 записей,
minimal — 400; `resolvedHistory` 200/50) хранит у записи только `id, entityType, recordType, source, status,
oldName, newName, fsName, projectName, confidence, resolution` (**без `fingerprint`, `entityId`, `separatorId`,
`proxyModId`, полей донора**).

## 5. Расхождения и вероятные ошибки

Отсортированы по весу. «Уверенность» — высокая, если механизм прочитан в коде; средняя — следствие выведено, но не
прогнано.

1. **Сверка сбрасывает решения по FS-записям.** `upsertSyncRecord` перезаписывает `status` (на `pending`) и `resolution`
   (на `''`) из нормализованного нового вызова (34 892), а сверка повторно создаёт запись на каждое ещё не устранённое
   расхождение при **каждом** пробеге (35 099, 35 123). Pre-commit сам запускает пробег. Итог: после pre-commit все
   FS-записи снова `pending` → фиксация закрыта; новый выбор решения меняет отпечаток → pre-commit устарел → новый
   пробег → снова сброс. `ignored` тоже не переживает следующий пробег. Фиксировать удаётся только проектные записи
   (и FS-записи, чьё расхождение исчезло само). Уверенность: механизм — высокая [код П2]; невозможность фиксации FS из
   модалки — средняя-высокая. Гонтлеты этого не ловят: они синтезируют `runs.surface` и сверку не запускают.
2. **`fs_rename_candidate` в живом пути не рождается**: cutover зовёт сверку с `{}` (134 083), подписей нет. Ветки
   `accept_fs_name` / `rename_disk_to_project` живы только в гонтлете. Высокая [код П2, `05-run`].
3. **Порядок операций без зависимостей и слияние записей с потерей исходного имени.** Операции идут в порядке
   `sync.records` (новые первыми, `unshift`), а повторный upsert по тому же отпечатку переписывает `oldName`. Сценарии
   (высокая по коду, сами сценарии не прогонялись):
   - переименовать дважды A→B→C: запись `B→C`, на диске `A` → `rename` даёт `missing` → «применено» → диск остаётся `A`,
     проект и modlist — `C`;
   - создать и переименовать (`project_created 'New Mod'` + `project_renamed 'New Mod'→X`): сначала rename (`missing`,
     «применено»), потом создаётся папка `New Mod`; в modlist — `X`;
   - переименовать и удалить мод: `move_to_deleted` ищет новое имя (`missing`, «применено»), затем rename переименовывает
     папку на диске — папка остаётся в `mods`, а из проекта мод ушёл;
   - создать и удалить до фиксации: `project_created` остаётся, папка создаётся для удалённого мода.
4. **Очистка до записи файлов** (37 315–37 337): записи удалены, journal и undo обнулены **до** записи `modlist.txt` /
   `project.json`; если запись упадёт, catch поставит `failed`, а `finally` сохранит autosave уже без записей и истории.
   Высокая [код П2; S03].
5. **Частичное применение без отката.** Ошибка на середине (`target_exists`, исключение FS) оставляет выполненные
   переносы и изменения проекта (`apply_project` меняет `state.project` в цикле), modlist не переписан, записи остаются.
   Повторная фиксация повторит `adopt` (будет skip) и т. п. «Откат» — только опись. Высокая [код П2; S22 rename-collision].
6. **Отсутствующий источник = «применено»** (`move_to_deleted`, `rename_folder` при `missing`/`skip`, `fs_*` при
   «не найдено»): запись исчезает без действия, след только в логе. Высокая [код П2].
7. **Перенос и переименование — копия всего дерева + `removeEntry`** (36 607). На гигабайтных модах медленно, не
   атомарно (при обрыве две копии), mtime не сохраняется — surface-подписи с mtime поменяются. Высокая [код П2].
8. **`project_restored` без операции**: запись создаётся `pending` (блокирует фиксацию), после подтверждения не
   применяется и не чистится — висит вечно. Высокая [код П2; гонтлет CP1 помечает `mod.restore.parked` как `contract-gap`].
9. **Донор**: `external_migration_pending` попадает в FS-ветку → `acknowledge_fs_record` → запись удаляется, папка
   донора **не переносится** (`contract-gap`, ADR 0006), а черновик-мод остаётся в проекте и пишется в `modlist.txt`
   (сериализатор его не фильтрует), папки под ним нет. Кроме того, сверка считает черновик сущностью проекта →
   `fs_deleted` на него. Высокая по коду для первых двух, средняя для `fs_deleted` (зависит от совпадения имён).
10. **`deleteModsToRecycle`**: `mod.enabled=false` до `wasEnabled` (112 454–112 455) → `originalEnabled` всегда false;
    восстановленный мод выключен. Дефолты восстановления расходятся (`?? true` в `restoreSyncParkedEntity` 112 323 против `typeof === 'boolean'`
    110 433). Высокая [код П2; S12].
11. **Сверка не закрывает исчезнувшие расхождения** (папку вернули руками — `fs_deleted` висит, `fsMissing=true`
    сбросится, а запись нет). Высокая [код П2].
12. **Три записи на одно переименование** (`fs_created` + `fs_deleted` + `fs_rename_candidate`) — если бы ветка
    работала; решения по ним независимы, можно выбрать противоречивые. Высокая [код П2].
13. **`sync_proxy_export_leak` по подстроке**: припаркованный `SkyUI` и живой `SkyUI - Flashpoint` → ложный блокер.
    Высокая [код П2].
14. **Компактная база autosave теряет `fingerprint` и `entityId`** (66 445): после восстановления из неё отпечаток
    пересчитывается по умолчанию → следующий upsert того же действия создаёт дубль; `fs_remove/rename_project_entity`
    не находят сущность по id. Средняя (зависит от того, из какой базы идёт восстановление). Записи сверх 1 200 / 400
    отрезаются (старые).
15. **Построитель плана пишет в проект** (`sync.commit.lastPlanAt/pendingCount/blocked`) на каждый рендер модалки.
    Высокая [код П2].
16. **`create_separator_folder` перезаписывает чужой `meta.ini`** без бэкапа; шаблон жёстко `gameName=SkyrimSE`. Высокая.
17. **`getRealSeparatorMods`** живая 59 029 не исключает `parked_real_mod` (мёртвая 34 743 исключала) — счётчики модов
    считают парковку. Высокая [S02].
18. **`ensureSyncStructure` на каждом чтении** перенормализует моды SYNC (новые объекты) — ссылки, взятые раньше,
    устаревают. Средняя [S02].
19. **CP5**: `USER_APPROVAL` формальный (`userApproved:true` по умолчанию), `verify` всегда ok, staging — no-op;
    `TARGET_REVISION` включает `projectUpdatedAt`, который меняет любой autosave. Мёртвый код, но в документах монолита
    описан как «авторитетный». Высокая [код П2].
20. Документы прохода 1 противоречат друг другу о живом пути (S21 против S03 §3.1) — снято в §2.0.

## 6. Что берёт NG и что меняет (ADR 0006, NG-D003)

Берём по смыслу (GOLD №1): записи-расхождения со статусами и решениями; блок фиксации при `pending`; прокси в
служебном разделителе; план операций со стадиями; pre-commit с отпечатком и свежестью; версия-бэкап; удаление =
перенос в `deleted/<ts>`; фиксация как граница версии; матрицу решений FS; правило «modlist без SYNC».

Меняем:
1. **Сверка — чистая функция** `reconcile(project, inventory, prev_signatures, planned) -> Vec<SyncRecord>` (ADR 0006).
   Она возвращает **полный набор** текущих расхождений; слияние с хранимыми записями — отдельный шаг с правилом:
   решение пользователя (`status`, `resolution`) **сохраняется**, пока не изменились факты записи (имена, подпись
   содержимого). Изменились факты → запись снова `pending`. Исчезло расхождение → запись `resolved` автоматически
   (`resolution: vanished`). Исправляет §5 п. 1 и 11.
2. **Одна запись на переименование.** Сверка, найдя пару «пропала сущность / появилась папка» с одной подписью
   содержимого (без имени папки, `sat-scan::signature` v2), выдаёт одну `fs_renamed {entity, from, to}` и **не**
   выдаёт `fs_created`/`fs_deleted` для этой пары. Подписи берутся из того же пробега (исправление 134 083).
3. **Проектные записи — по сущности, а не по событию.** Ключ `(сущность, вид)`; при повторной правке исходное дисковое
   имя (`disk_name`) **не перезаписывается**, меняется только целевое. Свёртка: created+renamed → created(конечное имя);
   renamed×N → renamed(первое → последнее); renamed+deleted → deleted(первое имя); created+deleted → запись исчезает;
   renamed обратно в исходное → запись исчезает. Исправляет §5 п. 3.
4. **План — чистая функция** без записи в проект; операции упорядочены по зависимостям, а не по времени записей
   (сначала освобождение имён: deleted/rename-from, потом rename-to/create), проверка коллизий целей до начала.
   `project_restored` и вкл/выкл — явные операции (или запись снимается). Неизвестный тип записи → блокер плана.
5. **Порядок исполнения** (исправляет §5 п. 4–6):
   1) ворота: нет `pending`, нет блокеров, свежий pre-commit (тот же отпечаток + `run_id` surface-задачи NG-D002 +
      ревизия проекта), цель не изменилась;
   2) `versions/<ts>/` + бэкап `modlist.txt` [plugins, loadorder] + `project.before` + **журнал намерения**
      (`commit.intent.json` с планом) — до любых изменений;
   3) применение операций по одной, после каждой — запись шага в журнал (`done`, `before/after`);
   4) запись `modlist.txt` [plugins, loadorder] и проекта через временный файл + атомарную замену (`*.tmp` → rename);
   5) только после успешной записи — закрыть записи (`resolved`, с id фиксации) и **затем** граница истории
      (undo-стек обнуляется или, лучше, получает метку «зафиксировано» вместо стирания);
   6) `commit.rollback.json` + отчёт; при ошибке на шагах 3–4 — **автоматический откат** по журналу в обратном порядке
      (переименования и переносы обратимы), записи остаются, проект возвращается к `project.before`.
   Отсутствующий источник — не «применено», а ошибка плана (обнаруживается проверкой до начала).
6. **Атомарный `rename` вместо копии.** Переименование в `mods` — `std::fs::rename` (один том). Перенос в `deleted/` —
   `rename`, если `deleted/` на том же томе. Если `mods` и профиль на разных томах — либо `deleted/` держать рядом с
   `mods` (решить в NG-D003, §7), либо копия → сверка размеров/хэшей → удаление источника, с журналом.
7. **`meta.ini` разделителя**: не перезаписывать существующий; шаблон — из MO2-адаптера (`sat-mo2`), игра из экземпляра.
8. **Сериализация modlist** — `sat-mo2::profile` по канону: без SYNC, прокси, припаркованных и черновиков донора;
   системные разделители — по правилам MO2 (область 3). Проверка утечки — по точному совпадению строк, а не подстроке.
9. **Донор**: операция плана `move_from_donor` с бэкапом (ADR 0006); запись донора не закрывается «подтверждением».
   Черновик не входит ни в сверку, ни в modlist до переноса.
10. **Восстановление** припаркованного сохраняет исходный `enabled` (п. 10 §5); «удалить» = снять мод из канона с
    записью, без превращения в псевдо-мод в UI-модели (прокси — только проекция).
11. **Хранение записей** — в контейнере проекта (ADR 0007) целиком, без усечения полей и количества.
12. **Один вход фиксации.** Нет `window`-обёрток и «теневых» ядер: команда `sync.commit` в `sat-app` (NG-D003), pre-commit —
    та же задача surface, что и обычный пробег.

## 7. Открытые вопросы

1. Где держать `deleted/<ts>` в NG: в профиле (как монолит, `profiles/<P>/monolith/deleted`) или рядом с `mods` ради
   атомарного `rename`, если тома разные?
2. `profile/project.json` после фиксации: читает ли его кто-нибудь в монолите (импорт? восстановление?) или это только
   снимок? В NG проект живёт в контейнере (ADR 0007) — нужен ли этот файл вообще?
3. `serializeModlist` пишет системные разделители (Skyrim — базовые моды игры, Overwrite, PLUGINS) и черновики донора
   как обычные строки; живая цепочка обёрток (логические группы 52 687–52 747) не прослежена — область 3.
4. Быстрые перестановки (`queueConflictReorderPersistenceV496`, телефон `reorderSeparatorsCanonicalCP19` /
   `phoneV2PersistSeparatorOrderCP17`) не создают `project_modlist_reordered` [S12]. Порядок всё равно пишется
   (finalize всегда переписывает modlist), но если других записей нет — блокер `no_active_sync_records` не даст
   зафиксировать только порядок. Проверить.
5. Вкл/выкл мода без записи: фиксация пишет `±` по проекту, но изменение enabled само по себе фиксацию не открывает
   (`no_active_sync_records`). Нужна ли в NG запись «порядок/вкл-выкл» (ADR 0006 говорит — да)?
6. Подтвердить сценарий §5 п. 1 живым прогоном на копии (если монолит ещё запускается): FS-запись → решение →
   pre-commit → статус.
7. Восстановление из компактной локальной базы autosave: насколько часто оно — главный путь (§5 п. 14, область 1).
8. Импорт `project.sync` монолита в NG (ADR 0007 п. 6): переносить ли записи (с учётом п. 1 и 14 §5) или
   пересчитывать сверкой при первом открытии.
