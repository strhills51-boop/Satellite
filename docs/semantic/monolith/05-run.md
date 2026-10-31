# 05 — Пробег анализов (сводный конспект области 5)

> Проход 2, сшивка. Источники: `pass/S15`, `S19`, `S20`, `S21` (основные), `S01` (сводка метрик), `S07` (окно пробега),
> `S11` (форматы, DDS VRAM, ворота метрик, legacy post-deep), `S03`/`S10`/`S12`/`S13`/`S14`/`S22` (точечно).
> Исправляет `docs/parity/run/00-overview.md` там, где проход 1 нашёл иное (помечено **[испр.]**).
> Точечные проверки по монолиту сделаны в этом проходе (помечено **[пров. П2]**).
> Строки — монолит `index.html` (196 155 строк). «IIFE» — главное замыкание приложения `(() => {` 19456 … `})();` 166057.

## 1. Коротко

Пробег — задача «прочитать папку `mods` и Overwrite, построить факты модов (пути, архивы, плагины, метрики, VRAM, теги,
стикеры) и конфликты» в двух режимах: «Поверхность» (`surface`, только сигнатуры корня + сверка SYNC) и «Бездна» (`full`).
Живой путь от кнопки: `runAnalysisPipeline` → Run Kernel recovery CP4 intake (жизненный цикл и Run Store) → исходная
`analyzeModsDirectory` 134655 → `runOrchestratedAnalysisCutoverV503` 134039. Для `surface` cutover сразу уходит в старый
`analyzeModsDirectoryLegacyV503` 132388; для `full` — в оркестратор v503: surface-проход, планировщик трёх маршрутов
(`scan / indexed / reuse` по возможностям артефакта), staging-поколение в IndexedDB, один ограниченный обход CP7 с анализаторами
CP8 (агрегаты), CP9 (плагины), CP15 (DDS VRAM) и проекцией CP11, печати, **авторитетный IDB-финализатор конфликтов**
(потоковый CP10 выключен с v508), promote/abort, проекции после promote. Весь legacy post-deep (`runPostDeepStages` 104539 и
его стадии) недостижим. Вычисления — legacy-код главного скрипта; ядро recovery (CP2–CP12) владеет только жизненным циклом,
событиями и окном пробега. Поверх цепочки — сводка метрик v599–v639 с печатью (seal) и воротами полноты.

## 2. Живой путь вызовов от кнопки до finalize

Обозначения: **[W]** обёртка/переназначение; **[жив.]** выигрывающая версия.

0. **Важная поправка [испр.][пров. П2]: переназначения `window.X = …` из recovery-скриптов НЕ перехватывают вызовы внутри IIFE.**
   S15 §2 (134655), S20 «Тонкость», S21 §3.2 утверждали: «top-level `function` классического скрипта — свойство `window`,
   поэтому `window.X = …` меняет голый идентификатор во всех скриптах». Это неверно: `analyzeModsDirectory`, `runSyncPrecommitCheck`,
   `executeSyncCommitPlan`, `validateExternalDonorSession`, `createExternalMigrationRecords` объявлены **внутри** стрелочного IIFE
   (xref `depth=1`; строки 19456 `(() => {` и 166057 `})();` проверены). Голые вызовы внутри IIFE (35963, 36424, 36463, 37522,
   107695, 108139) резолвятся в **локальную** привязку = оригиналы; `window.*` они видят только через мост `Object.assign(window,…)`
   134686. Следствия: (а) единственный путь пробега через CP4 intake — явный `window.ModlistToolAuthoritativeAnalysisIntakeV503.run`
   из `runAnalysisPipeline`; (б) вложенный surface-пробег из SYNC precommit (35963) идёт **мимо** CP4 — Run Store его не видит;
   (в) CP5 SYNC issuance и CP6 donor-обёртки достижимы только для вызывающих через `window.*` (поздние скрипты, консоль) — их
   «живость» для UI нужно пересмотреть в областях 11/12. Именно поэтому `runAnalysisPipeline` зовёт intake явно через `window`.

1. **Кнопка** «Запустить пробег» в модалке `openAnalysisModal` 111207 (S12) → `await runAnalysisPipeline()` 111477.
   Модалка: `analysisConfig` по умолчанию `{deepEnabled:true, compactBuildEnabled:true, archiveScanEnabled:true,
   tagRulesEnabled:false, stickerRulesEnabled:false, customArmed:false}`; кнопки для `customArmed` в разметке нет (мёртвый wire,
   S12) → **custom-стадия в живом UI не включается никогда**.
2. **`runAnalysisPipeline`** 134938–134978 [жив., единственное объявление]: `archiveScanEnabled` по умолчанию `true`;
   `getModsPathValidation` (score<50 → confirm); `getOverwritePathValidation` обязателен при `deepEnabled !== false`;
   `startRunSession(…, cfg)` (69231); `mode = cfg.deepEnabled ? 'full' : 'surface'`; затем **только**
   `window.ModlistToolAuthoritativeAnalysisIntakeV503.run(mode)` («отказ обходить Run Kernel»). V520 177173 проверяет это по тексту.
3. **CP4 intake `run(...args)`** 175322 (скрипт recovery CP4, owner 175271, `VERSION 'v503-authoritative-analysis-intake-cp4'`) [W вокруг 134655]:
   маршрут `analysis.intake.cp4`, kind `intake`, домен `analysis`: `discovery` (capability, evidence `analysis.source-observed`) →
   `preflight` (barrier, все факты pass) → `pipeline` (capability `analysis.pipeline`: ставит
   `window.__modlistToolAuthoritativeAnalysisActiveRunV520 = {runId,…,mode}`, artifact `analysis.result` pending, evidence
   `runtime.telemetry`; **ровно один** вызов доверенного исполнителя; снимает active-run; workItems из
   `ModlistToolRunSessionStateBridgeV524.snapshot().stages`; artifact ready со значением результата) → `verification` (всегда ok) →
   `promotion` (claim). commitBoundary `{id:'analysis-promotion', physical:false}`. Возвращает **ровно результат legacy-конвейера**.
   Доверенный исполнитель 175285: `LegacyCP12.legacyEntrypoint || .entrypoint || window.analyzeModsDirectory`; в мосте
   `ModlistToolAnalysisCutoverRuntimeV503LegacyCP12` 134673 поля `legacyEntrypoint` нет → `entrypoint` = исходная 134655.
   `window.analyzeModsDirectory = run` 175411 влияет только на `window`-вызывающих (см. п. 0).
4. **`analyzeModsDirectory(mode='surface')`** 134655 [исходная, IIFE] → `runOrchestratedAnalysisCutoverV503(mode)`.
   `window.ModlistToolAnalysisCutoverV503.run` (CP12, 173757) = это же значение, захваченное в 134674 → обходит CP4
   (вызывающих в коде нет; консоль).
5. **`runOrchestratedAnalysisCutoverV503(mode)`** 134039–134653 [жив.]:
   - `mode !== 'full'` → **`analyzeModsDirectoryLegacyV503(mode)`** 132388 (ветка «Поверхность», п. 6).
   - иначе требует API `ModlistToolAnalysisGenerationV503`, `LiveRunSessionV503`, `DynamicArchiveSourcesV503`,
     `StreamingAggregatesV503`, `PluginAnalyzerV503` (нет → throw). Шаги 5.1–5.15 ниже.
   - 5.1 **Sync/discovery** 134059–134094: обязательны `state.fs.modsHandle` и `overwriteHandle`; `readModsEntriesFromHandle`
     134696 (только подпапки, `localeCompare base`); `buildSyncReconcileResult(folderNames, {})` 134083 [пров. П2] — **пустой
     surface**: сверка переименований по `signatureBuckets` в «Бездне» не может сработать.
   - 5.2 **Идентификаторы** 134101–134119: `authorityRef = getCurrentAnalysisRunRef(true)`; `previousRunId = authorityRef.runId`;
     `stagingRunId = "analysis-v503:" + Date.now().toString(36) + ":" + random(8)`; счётчики v504 `begin`; предыдущие сигнатуры =
     `runs.surface.signaturesByName` ⊕ IDB `getSurfaceSignaturesForModNames`; `previousMetricSummaryV612 =
     compactAnalysisMetricSummaryForSnapshotV599(runs.metrics)`.
   - 5.3 **Surface** 134121–134151: `runSurfaceStage(entries, mode, previousSignatures)` 134708 (watchdog-лог 5 с, v511).
     **[испр.]** 00-overview говорит «surface из единого обхода» — нет: `buildCutoverSurfaceResultV503` 132870 нигде не вызывается
     (только экспорт и самотест CP12); отдельный проход корня остаётся.
   - 5.4 **Route planning** 134152–134172: `repairPromotedSurfaceSignatureArtifactsV513` 133395 → `buildConfirmedReuseRouteV506`
     133454 → `blocked.length>0` → throw `CP3 blocked before deep…` (до создания поколения). `traversalEntries` = scan-моды +
     `buildIndexedTraversalEntriesV516` (поддельные handles) + `[OVERWRITE_MOD_NAME, overwriteHandle]`; reuse в обход не идут.
     `createFrozenPreDeepRoutePlanV505` 133344 — исторический слой (всегда scan), в живом плане не участвует.
   - 5.5 **Staging** 134174–134195: `generationApi.createCurrentProjectGeneration({…metadata{checkpoint:'cp12-cutover-regression',
     physicalModScanCount, indexedModAnalysisCount, confirmedReuseCount, preDeepLayerOrder, frozenRouteCounts}})` → `getWriteRef` →
     `materializeConfirmedReuseArtifactsV506` 133631 (физическое копирование путей reuse-модов в staging).
   - 5.6 **Живой обход** 134196–134258: `archiveReader` (выкл. архивы → `async () => []`, иначе
     `ModlistToolAnalysisRuntimeInputsV503.readArchiveManifestEntries(file,{includeByteLengths:true})`, фолбэк `…Paths`);
     `collectCutoverOwnerMetaV503` 132840; `createDdsVramCollectorV613({modNames, archiveScanEnabled})` 102756.
     `liveApi.runShadow({persist:true, authoritativeRunId: previousRunId, stagingRunId, ownerMetaByModName, archiveReader,
     isConflictPathIgnored, onProjection: onCutoverProjectionV503, configureKernel → ddsVram.register(runtime), лимиты §4.9,
     authoritativeConflictFinalizerOnly:true})`. Имя `runShadow` историческое — это авторитетная запись в staging.
     Вложение: CP11 `runShadow` 173570 ⊃ CP10 173054 (заглушка `cp10-staging-only.v1`) ⊃ CP9 172463 ⊃ CP8 171761 ⊃
     CP7 `DynamicArchiveSources.runShadow` 171151 (одно ядро CP3, sink CP6 `createPathStorageSink` 169914 → IDB `modPaths`).
     Порядок регистрации анализаторов в ядре = порядок диспетчеризации: `cp07.loose-route-stream` → `cp07.archive-source-expander`
     → `cp07.virtual-route-stream` → `cp08.streaming-aggregates` → `cp09.loose-plugin-header` → [CP10 пропущен] → CP11
     `session.attach` (не анализатор) → `cp15.dds-vram-metrics`. Прогресс deep: `10 + 80·complete/(scan+indexed)`, ≤90 %.
   - 5.7 **Перепечатка indexed** 134260–134284: запись staging для indexed-модов переписывается с `artifactRoute:'indexed'`,
     `source:'cp6-indexed-source-producer-v1'`, `indexedFromRunId`, `physicalDirectoryEnumerated:false`, `targetedContentLookup:true`, hot-only.
   - 5.8 134286–134318: `finalizeUnifiedStagingLedgerV507` 133803; upstream через `findCompletedOrchestratorRunV503`;
     **любой упавший архив при `archiveScanEnabled !== false` → throw** 134300–134306 [пров. П2] (`CP12 archive analysis failed for N
     source(s)`) — поколение abort. `deepResult{storage:'indexeddb-paths', total, scanned, indexed, skipped=reused, reusedPaths,
     pathsWritten (без overwrite), changedMods, indexedMods, orchestratorCutover:true}`.
   - 5.9 **Печать возможностей v517** 134320–134395 (см. §4.4).
   - 5.10 134396–134412: `verifyThreeRouteAcceptanceV518` 133979, `sealSurfaceSignatureArtifactsV513` 133395+,
     `putMeta(status:'staging-complete', conflictTruth:'pending-authoritative-idb-finalizer', три печати)`.
   - 5.11 **Конфликты** 134414–134453: `AnalysisPathStore.rebuildOverwriteOwnersFromPaths` 120968 [пров. П2] →
     `rebuildOverwriteChainsAndContextsFromOwners` 120780; проверка `conflictParity` (§4.5). Любое false → throw.
   - 5.12 **Commit** 134455–134490: `createCutoverRuntimeWitnessV503` 132932 → `canCommitReady` иначе throw →
     `updateAuthorityPatch` → `markCommitReady` → `promoteCurrentProjectGeneration({persistProject: async()=>true,
     autosaveReason:'cp12-orchestrator-promote'})` (CP4 generations 168902).
   - 5.13 **После promote** 134492–134549 (ошибки поколение уже не откатывают): `commitSurfaceResults` 154234,
     `commitDeepResults` 154329, `commitCutoverArchiveProjectionV503` 132949, `commitCutoverPluginProjectionV503` 132995,
     `loadCutoverConflictMetricFactsV645` 102387, `applyCutoverAggregateProjectionV503` 133004 (метрики + печать сводки),
     `ensureOverwriteSystemSeparator`, `paths.modsPath/overwritePath`, `window.modlistToolMaterializeOverwriteNormalizedLayerV501
     ({activate:true})` [жив. версия — v660 149559, S14] (ошибка → warn), `runCustomStage` 104466 (гейт `deepEnabled && customArmed`),
     `runLogicalGroupProductLifecycleV557` (ошибка → warn), `runPostDeepAutoColdMigration` 128482, `updateDerivedFromRunsToMods` 132181,
     `cleanupCompletedRunRuntime`, `recordHistory('run_full')`.
   - 5.14 134551–134601: `runs.treeFull.orchestratorCutover` (schema `…cutover-manifest.v1`) → `window.__modlistToolCP12LastRun`;
     финальный autosave; `finishRunSession` 69758 («Пробег завершён»); v504 `finish('promoted')`.
   - 5.15 catch 134602–134652: v504 `finish('failed-not-promoted')`; отмена (`isAnalysisMetricCancellationV637`) →
     `cancelAnalysisMetricSummaryV637`, иначе `failAnalysisMetricSummaryV636`; поколение создано и не promoted →
     `generationApi.abort({reason:'cp12-cutover-cancelled|failed:…'})`; `cancelRunSession`/`failRunSession`; alert (не при отмене); rethrow.
6. **Ветка «Поверхность»** `analyzeModsDirectoryLegacyV503(mode)` 132388–132819 [жив. только для `surface`]:
   подавить autosave → проверки handles → `setRunPreliminaryUiV503('sync'|'surface')` → `readModsEntriesFromHandle` →
   **теневые прогоны** (try/catch, только лог): `ModlistToolRouteSourceSchedulerV503.runLegacyShadow` (CP5) **без флага, на каждом
   surface-пробеге** (132433 [пров. П2]); CP6–CP11 — под флагами `__modlistToolCP0NShadow…`/`modlistToolFlags.cp0N…`, старший гасит
   младшие, `persist:false` → предыдущие сигнатуры → `runSurfaceStage` → `buildSyncReconcileResult` (здесь **с** surface-результатом,
   missingInProject/missingOnDisk после surface) → `commitSurfaceResults` → `AnalysisPathStore.putModAnalysisStates` → (ветка
   `full` с `runDeepStage` 134851 / `runPostDeepStages` 104539 — **недостижима**) → `updateDerivedFromRunsToMods` →
   `cleanupCompletedRunRuntime` → `recordHistory('run_'+mode)` → autosave → `finishRunSession`; ошибка → `failRunSession` + alert.
7. **Окно пробега и приёмка** (наблюдаемое состояние): `startRunSession` → стадии `buildRunSessionStagesForConfig` 69436;
   проекции CP11 → `onCutoverProjectionV503` → `applyRunSessionRuntimeProjectionV503` (69383 → owner 69209) →
   `publishAuthoritativeAnalysisRuntimeV520` 69174 / `publishAuthoritativeAnalysisWorkV520` 69140 → Run Store CP2 → адаптер CP11
   recovery 176007 → пейджер CP12 recovery 176605 (`followLatest('analysis','intake')` 177162). По терминалу контракта —
   **приёмка V522** 177340 (`validateRun`, полный аудит IDB, самопереход `completed→completed`). Кнопка «Принять» →
   `closeRunSessionAndFinalize` (69758+) → `renderAll`.
8. **Другие входы** в пробег: SYNC precommit `runSyncPrecommitCheck` 35938 → `startRunSession('Pre-commit sync check', {deepEnabled:false,…})`
   → `await analyzeModsDirectory('surface')` 35963 (мимо CP4, п. 0); донор `validateExternalDonorSession` 107398 → `runSurfaceStage`
   (сигнатуры донора, `LEGACY.signatureFreeze` 165531); консоль — `ModlistToolAnalysisCutoverV503.run`, CP4 `run`.
   Ручной «перестроить конфликты» 154290 → `runOverwriteOwnersStage([])` 104274 (+ повторный rebuild цепочек — двойная работа).

## 3. Модель данных

### 3.1. Словарь оркестратора (CP2 167355, `modlisttool.analysis-orchestrator.vocabulary.v1`)
Все сущности frozen, `schema …entity.v1`, строки trim, пустые обязательные → TypeError.
- `RunGeneration` 167462: `projectId, runId, previousRunId?, status (staging), createdAt, metadata`.
- `RouteSource` 167477: `sourceId, kind, ownerId, ownerName (=ownerId), parentSourceId?, origin ('loose'), status ('queued'), metadata`.
- `Observation` 167494: `observationId, runId, sourceId, ownerId, origin, kind, normalizedPath?, extension (lower), size (finite|null),
  modifiedAt (string|null), provenance`. Живые kind: `loose-file` (origin `loose`), `virtual-file` (origin `archive`).
  provenance CP7: `{modName, depth, rootLevel, archiveSourceId, metadataRead}`; виртуальные — плюс V555/V613 `byteLengthStatus,
  packedByteLength, compressed, archiveFormat, ddsHeaderBytes, texturePayloadBytes, textureWidth/Height/MipCount/Format`.
- `WorkUnit` 167513: `workId, analyzerId, sourceId?, kind, status, payload`. `Sink` 167528: `sinkId, kind, mandatory (true), metadata`.
- `BarrierToken` 167540: `tokenId, barrierId, ownerId, sourceId?, status ('open'), openedAt, metadata`.
- `RunEvent` 167556: `sequence (int ≥0), at, type, phase?, sourceId?, analyzerId?, barrierId?, payload`.
- `RuntimeSnapshot` 167574: `runId, phase ('idle'), revision, sources, work, barriers, analyzers, queues, counters, issues[]`.
- Перечисления: `EXECUTION_CLASS {stream-consumer, source-expander, barrier-finalizer}`; `FAILURE_POLICY {mandatory, optional,
  degraded}`; `RUN_GENERATION_STATUS {staging, authoritative, aborted}`; `SOURCE_STATUS = WORK_STATUS {queued, active, complete,
  error, cancelled}`; `BARRIER_STATUS {open, closed}`; `CONTRACT_ID_RE = /^[a-z0-9][a-z0-9._:-]*$/i`.
- **Контракт анализатора** (`validateAnalyzerContractV503` 167607): ключи только `id, executionClass, failurePolicy, accepts, consume,
  flush, finalize, metadata` (иначе `CONTRACT_KEY_UNSUPPORTED`); defaults stream-consumer / mandatory; `accepts`, `consume`
  обязательны. Результат `consume` (`normalizeAnalyzerResultV503` 167592): только `{facts, sources, work, warnings}`.
  Реестр 167645: порядок `list()` = порядок регистрации = порядок диспетчеризации.

### 3.2. Ядро CP3 (167775) — фазы и очереди
- Фазы `idle, running, draining, complete, failed, cancelled`. Очереди: sources, observations, work, route-writes. Счётчики:
  `sourcesQueued/Started/Completed/Failed, observationsQueued/Dispatched, workQueued/Completed/Failed, backpressureSignals,
  warnings, errors, yields`. Барьер = набор токенов (`createBarrierLedgerV503` 167916); закрыт, когда закрыты все.
- Ядро пишет `status:'failed'` у source/work (словарь знает `error`) — см. §5.
- Барьер CP7 `routes.complete`: токены `${runId}:physical-root`, `${runId}:storage-flush` и по одному на архив
  `${runId}:barrier-token:${hash(sourceId)}`.

### 3.3. Поколение (CP4 168499, IDB-store `analysisGenerations`)
- Запись `normalizeGenerationRecordV503` 168560: `{schema 'modlisttool.analysis-orchestrator.generation.v1', key=projectId::runId,
  projectId, runId, previousRunId|null, status, createdAt, updatedAt, commitReadyAt|null, promotedAt|null, abortedAt|null,
  abortReason, promotionError, authorityPatch, metadata}`.
- Переходы: `begin` → staging; `updateAuthorityPatch`, `markCommitReady` — только staging; `promote` staging → authoritative;
  `abort` staging → aborted (+cleanup); authoritative не прерывается; сбой promote → откат указателя, остаётся staging с `promotionError`.
- Авторитетный указатель `AUTHORITY_FIELDS_V503` 168607: `projectId, runId, storage, analysisStore, overwriteOwnersStore,
  overwriteChainsStore, overwriteChains, overwriteNormalizedChains, conflictIndex, analysisGeneration`. Patch допускает
  `analysisStore, overwriteOwnersStore, overwriteChainsStore, overwriteChains, overwriteNormalizedChains, conflictIndex`.
- Чтение авторитета для метрик — `resolveProjectAnalysisRunAuthorityV616` 20939 (4 кандидата, §4.8).

### 3.4. Run Kernel recovery: контракт пробега (CP2 174165, `modlisttool.authoritative-run-contract.v1`)
- 12 статусов: `created, planning, running, blocked, staging, awaiting-commit, committing, verifying, promoting, completed,
  failed, cancelled`; терминалы — последние три. Таблица переходов `AUTH_ALLOWED` 174188 (самопереход разрешён всегда):
  created→{planning, running, blocked, cancelled, failed}; planning→{running, blocked, staging, cancelled, failed};
  running→{planning, blocked, staging, verifying, cancelled, failed}; blocked→{planning, running, staging, awaiting-commit,
  cancelled, failed}; staging→{blocked, awaiting-commit, committing, cancelled, failed}; awaiting-commit→{blocked, committing,
  cancelled, failed}; committing→{verifying, failed}; verifying→{promoting, failed}; promoting→{completed, failed}.
- Поля контракта (S21 §4.1): `schema, version, runId, kind (intake|issuance), domain, status, route{routeId, currentStepId,
  steps[{id,type}], liveStageId?}, workItems[{workId, type, capability, schemaVersion?, status, opaque?, payload?,
  requiredCapabilities[], resultSummary{legacyStageId, percent, current, total, text, statusLabel, source}}], artifacts[{artifactId,
  type, status (pending|ready|partial|blocked|failed|available), value}], evidence[{evidenceId, type, status (observed|verified|failed),
  data|value}], barriers[], claims[{claimId, predicate, state proven|unproven}], revisions{field:n}, commitBoundary{id, state, ready,
  physical, adapter?, delegatesTo?, externalPhysicalCommit?}, diagnostics{warnings, errors, recent[≤120], cp7Acceptance?}, scope,
  legacyProjection, authority{sourceOfTruth:'run-store', lifecycleOwner, importedFromLegacy, compatibilityReason, storeVersion,
  storeRevision}, revision, createdAt, updatedAt, completedAt, transitionHistory[≤240 {from,to,reason,at}]`.
- runId intake анализа: `${domain}-${kind}:${base36 time}:${seq}` (`authIdentity` 174242). Это **не** runId поколения
  (`analysis-v503:…`); V522 сверяет хранилище по runId поколения из `bridge.authorityRef()`.

### 3.5. Виды барьеров
- **Ядро recovery CP3** 174585 (13): `HANDLE_PERMISSION, SOURCE_IDENTITY, SOURCE_REVISION, TARGET_IDENTITY, TARGET_REVISION,
  CAPABILITY_AVAILABLE, DEPENDENCIES_READY, WORK_DRAINED, ARTIFACTS_READY, USER_APPROVAL, STAGING_COMPLETE, COMMIT_ALLOWED,
  POST_COMMIT_VERIFIED`; pre-commit — первые 12. Для intake waived: TARGET_IDENTITY, TARGET_REVISION, USER_APPROVAL,
  STAGING_COMPLETE, COMMIT_ALLOWED. Состояния факта `open | blocked | passed | waived` (open блокирует).
- **Оркестратор** (данные, не ядро recovery): один барьер `routes.complete` (§3.2).
- **v526Raw эффект-барьеры** (12, область 13) к пробегу анализа не относятся.

### 3.6. Модель возможностей (capability)
Два разных понятия с одним словом:
1. **Возможности артефакта мода** (`artifactCapabilities`, schema `…artifact-capabilities.v7`, пишутся в `modAnalysisState`):
   `loosePaths, routeMetadataV2, metrics, ddsVramV613` — всегда требуются; `archiveVirtualPaths, archiveEntrySizesV555` — при
   `archiveScanEnabled !== false`; `tags`/`stickers` — если соответствующие правила включены. Основа маршрутов (§4.2).
2. **Capability Registry** recovery CP8 175890 (`window.RunCapabilityRegistry`): ключ `type@schemaVersion`, regex типа
   `/^[a-z][a-z0-9_-]*(?:\.[a-z0-9][a-z0-9_-]*)+$/i`, каталог из 17 объявленных типов (`analysis.discovery/surface/deep/post-deep`,
   `sync.*`, `donor.*`, `config.*`, `behavior.*`). При загрузке пуст; первый `register` — `registerKernelCapabilities()` 182544
   (6 `sync`-заглушек `{ok:true, compileOnly:true}`, S22). Шаги intake анализа несут свой `step.execute` — реестр не используется.
   Политика неизвестной способности: blocked work item `opaque:true`, исполнение никогда; восстановление `recoverRun` → resume.
3. **Composite capability** логических групп (v541–v554: `paths, tree, aggregates, overwrite, routes…`, `requiredForCutover`) —
   область 15, в пробеге не участвует, кроме `runLogicalGroupProductLifecycleV557` после promote.

### 3.7. План маршрутов (строка на мод, frozen; schema `modlisttool.analysis-predeep-route.v3`, checkpoint 6.1)
`{modName, signatureStatus, routeKind ∈ FULL_SCAN|INDEXED_ANALYSIS|FULL_REUSE, executionRoute ∈ scan|indexed|reuse|blocked, policy,
pathSource ∈ physical-scan|promoted-index, missingCapabilities[], issues[]}`; `counts{total, fullScan, indexedAnalysis, fullReuse,
reuse, scan, indexed, blocked, syncMissing*}`. Возврат планировщика `{plan, reuseArtifacts: Map, blocked[]}`.

### 3.8. Факты на мод (что производит живой пробег)
- **Staging IDB** (`modPaths`/`modPathChunks`): пути мода (CP7, **в нижнем регистре**, §5) + `routeMetadata` на путь
  (`originKind loose|archive|archive-ambiguous|ambiguous, surfaceRole content-file|archive-container|archive-entry, byteLength,
  modifiedAt, contentHash, sourceRef, sourceCount, shadowedSourceCount, metadataStatus`); манифест `{key, storage:'chunks',
  chunkStore:'modPathChunks', pathStorageSchema:'path-storage-manifest-v1', pathStorageMode:'idb-chunks', pathStorageHotCopy:true,
  pathCount, samplePaths[≤40], pathContentHash, artifactRoute, source, scannedInRunId|reusedFromRunId|indexedFromRunId, …}`.
- **`modAnalysisState`**: `{modName, surfaceSignature, previousSurfaceSignature, signatureStatus, rootEntryCount, rootDirs,
  rootFilesSample (≤40), rootFileExts, changed, pathsStored, lastSurfaceRunAt, lastSeenAt, surfaceStatus, artifactRoute,
  analyzerExecutionSchema:'modlisttool.capability-specific-execution.v1', analyzerCapabilitiesAdded/Preserved,
  physicalPathIndexComplete, physicalPathIndexSchema 'modlisttool.physical-path-index.v1', physicalPathIndexSignature,
  artifactCapabilities, surfaceSignatureArtifactSchema?}`.
- **Агрегат CP8** (`…mod-stream-aggregate.v1`, `finalizeModV503` 171643): `modName, physicalBytes, physicalSizeMb (bytes/2^20,
  2 знака), physicalFileCount, physicalSizeUnknownCount, virtualFileCount, routeCount, extensionCounts{physical,virtual,all},
  metricCounts (v602 `{v:2,a,p,x}`), pathFacts (v607 `{v:1,d,p,x,s,sp,sx,o}`), candidates{pluginCount, archiveCount, dllCount,
  configCount, *Samples}, compactFacts{folders, rootDirs, tags, stickers, pathSamples}, digest`.
- **Плагины CP9**: `pluginsByModName[mod] = {lastRunAt, status:'done', plugins[], archiveVirtualPluginScan:false}`, запись
  плагина = форма `parsePluginHeaderFromFile` 100666 (S11 §2) + `recordScanStatus`.
- **DDS VRAM** (`modlisttool.dds-vram-collector.v1`, version 613): `byModName` со счётчиками ddsCount/known/unknown/vramBytes/loose/
  archive/контейнеры, корзины разрешения, форматы, статус complete|partial|unavailable, coveragePercent.
- **Проекции в `project.runs`** после promote: `surface{foldersSeen, missingInProject, missingOnDisk, changedSignatures,
  signaturesByName}`; `treeFull{tagsByModName[mod]={tags, stickers, folders, exts, sizeMb, fileCount=routeCount, physicalFileCount,
  archiveVirtualFileCount, dirCount}, analysisStore, overwriteOwnersStore, overwriteChainsStore, overwriteChains/conflictIndex
  (пустые v2-оболочки «живут в IndexedDB»), orchestratorCutover}`; `archiveScan{archivesByModName[mod][]={name, relativePath,
  status, error, kind, virtualPathCount, virtualByteLengthKnownCount, virtualUnpackedBytes, entrySizeCoverageStatus, samplePaths,
  sampleEntries, sourceId, digest}, pathsByModName (≤40 образцов!), stats, status 'done'|'disabled', normalizationVersion,
  orchestratorSource:'cp12-single-traversal'}`; `pluginScan` (клон CP9); `metrics` (сводка v599).
- **`mod.derived`** (`applyCutoverAggregateProjectionV503` + `updateDerivedFromRunsToMods`): `sizeMb=physicalSizeMb,
  fileCount=routeCount, physicalFileCount, archiveVirtualFileCount, folders, exts, dirCount, tags, stickers`; `analysisRef{surfaceSignature,
  lastSurfaceRunAt, lastTreeFullRunAt, lastPluginScanRunAt, lastTagRulesRunAt, lastStickerRulesRunAt, pathSource}`.
- **Конфликты** (IDB, авторитетно): `overwritePathOwners, overwriteModSummary, overwriteModFiles, overwriteChains, overwriteChainByMod,
  overwriteContexts, overwriteContextByChain, overwriteChainPathChunks, overwriteMeta` + проекции `overwriteModView,
  overwriteContextView, overwriteChainPreview, overwriteRouteGroups*, overwriteNormalized*` (полный список stores — S15 131475).

### 3.9. Формат сводки метрик (v599–v639, S01)
- Ведро `project.runs.metrics`: `{schema 'analysis-metric-summary-state/v2', projectId, runId, status ∈ idle|active|done|stale|error|
  cancelled, revision, lastRunAt, digest, terminal{status, at, reason≤2048, phase≤256, runId}|null, dictionaries{capabilities,
  revisions, timestamps, reasons, valueStrings, valueKeys}, byEntityKey{entityKey → {metricId → tuple}}}`.
- Кортеж записи `[statusCode, encValue|null, capIdx, revIdx, tsIdx, reasonIdx, semanticCode, confidence]`, хвост `-1/null/semantic 0`
  срезается до длины 2. Статусы `available0 pending1 unsupported2 unavailable3 error4`; семантика `fact0 heuristic1 user-confirmed2 unknown3`.
  Значения: `['@v:s', idx]`, `['@v:l', …]`, `['@v:m', keyIdx, val, …]`; пределы depth 4, list 64, map 64, строка 4096, ключ 128.
- Ключи сущностей: `mod:<id>`, `separator:<id>`, `project:<id>`, `logicalGroup:<id>`; составной мод — `mod:composite-mod:<groupId>`.
- Запись (до кодирования): `{status, semantic, confidence∈[0,1]|null, sourceCapability, sourceRevision, measuredAt, reason, value?, valueKind?}`.

## 4. Точные правила и формулы для переноса

### 4.1. Сигнатура surface (`buildAnalysisSignature` 78508, применение `runSurfaceStage` 134708)
- Один уровень `entries()` корня мода: каталоги, кроме имён на `.bsa`, → `rootDirs` (lower); файлы → `rootFiles` (lower) и
  `rootFileExts` (`.ext` lower); всё сортировано. `rootEntryCount` **включает** пропущенные `.bsa`-каталоги.
- `signature = [modFolderName, rootEntryCount, rootDirs.join(','), rootFiles.join(','), rootFileExts.join(',')].join('|')`.
  Фикстура CP1 165779: `{Alpha, 4, [meshes,textures], [meta.ini,plugin.esp], [.esp,.ini]}` → `Alpha|4|meshes,textures|meta.ini,plugin.esp|.esp,.ini`.
- Статус: нет прошлой → `new`; ≠ → `changed` (только он в `changedSignatures`); иначе `unchanged`.
- Политика `fully-frozen` (CP1 `signatureFreeze` 165531): потребители — reuse-планировщик, `modAnalysisState.surfaceSignature`,
  `signatureBuckets` SYNC, донор. В NG — сознательное расхождение (v2: без имени папки, глубина 2, размеры, mtime).
- `nextFullTags[mod]` surface-прохода: `{tags:[], folders: rootDirs, exts: rootFileExts, sizeMb:0, fileCount:0, dirCount:0}`.

### 4.2. Планировщик трёх маршрутов `buildConfirmedReuseRouteV506(entries, surfaceResult, syncResult, projectId, previousRunId, analyzerConfig)` 133454
Ворота метрик v632: `prepareGenerationMetricReuseGateV632(project, previousRunId)` + на мод `evaluateGenerationMetricEntityForFullReuseV632`.
Решение на мод, по порядку:
1. `signatureStatus !== 'unchanged'` → `FULL_SCAN/scan`, policy `cp6-new-or-changed-full-scan`, `pathSource:'physical-scan'`.
2. `unchanged`, но нет `previousRunId` / нет `modPaths` / нет `modAnalysisState` / пути не читаются (`getModPaths(…,{cache:false,
   returnReadDiagnostic:true, strictColdRead:false}).ok===false`) / нет артефакта сигнатуры / сигнатура не совпала →
   `executionRoute:'blocked'`, policy `cp6-promoted-physical-index-unavailable`, коды `missing-promoted-generation, missing-modPaths,
   missing-modAnalysisState, unreadable-modPaths:<reason>, missing-surface-signature-artifact, surface-signature-artifact-mismatch`.
   Любой blocked → throw до создания поколения.
3. `physicalPathIndexComplete !== true && capabilities.loosePaths !== true` → `FULL_SCAN` (`cp6-incomplete-promoted-physical-index`).
4. Есть недостающие требуемые возможности (или метрик-гейт `complete !== true` → добавить `metrics`) → `INDEXED_ANALYSIS/indexed`,
   `missingCapabilities`, `pathSource:'promoted-index'`, артефакт в `reuseArtifacts`.
5. Иначе `FULL_REUSE/reuse` (`cp6-complete-promoted-artifact-reuse`). Пустой мод (pathCount 0) — валидный reuse.
- `indexed`: `createIndexedDirectorySourceV516(modName, realHandle, physicalPaths)` — поддельный DirectoryHandle по путям прошлого
  поколения (`entries()` отсортированы `localeCompare base`), `getFile()` — в реальный handle по требованию. Обход не перечисляет
  каталоги, но **все** анализаторы гоняются заново по всем путям мода (досчитывается не только недостающая возможность — см. §5).
- `reuse`: `materializeConfirmedReuseArtifactsV506` — копия путей в staging с `source:'cp7-reuse-hot-materialization-v519'`,
  `artifactRoute:'reuse'`, `reusedFromRunId`, hot-only (`pathStorageForceHotOnly`, `pathStorageMode:'idb-chunks'`, `coldSegmentRef:null`,
  `coldSegmentStatus:'none'`), `routeMetadata` из прошлого поколения (ошибка → null); `modAnalysisState{signatureStatus:'unchanged',
  artifactRoute:'reuse', reusedFromRunId, lastSeenAt}`.
- Горячий контракт v519 (`normalizeHotPathStorageWriteExtraV519` 113998 / мост 124501): reuse/indexed — только hot; dual-write
  только по явному запросу с валидной cold-ссылкой.

### 4.3. Реестр staging (v507) и приёмка маршрутов (v518)
- `finalizeUnifiedStagingLedgerV507(projectId, runId, currentModNames, overwriteName)` 133803: allowed = моды + overwrite;
  лишние → `stale`, отсутствующие → `missing`, дубликат → throw; stale|missing → throw `CP4 staged artifact set mismatch`.
  Маршрут записи: overwrite → `scan`; иначе `rec.artifactRoute || (reusedFromRunId ? 'reuse' : 'scan')` ∈ {scan, reuse, indexed}.
  `counts{artifacts, scanMods, reuseMods, indexedMods, overwriteArtifacts, scannedPaths, reusedPaths, indexedPaths, totalPaths}`.
- `verifyThreeRouteAcceptanceV518` 133979: нет дублей/неизвестных; `FULL_SCAN` ⇒ `scan` и в обходе; `INDEXED_ANALYSIS` ⇒ `indexed`,
  в обходе, `unchanged`, `promoted-index`, staging `physicalDirectoryEnumerated===false ∧ targetedContentLookup===true`;
  `FULL_REUSE` ⇒ `reuse`, **не** в обходе, `unchanged`, `promoted-index`; `counts` плана = печати возможностей
  `{indexed, reused, scanned}`. Иначе throw (≤32 причин).

### 4.4. Печати перед promote (инварианты)
1. **Сигнатуры v513**: `repairPromotedSurfaceSignatureArtifactsV513` — backfill сигнатуры в прошлое поколение для `unchanged`, только
   если legacy-сигнатура ровно равна текущей и есть state + `modPaths` (`…BackfillSource:'promoted-project-surface-exact-match-v513'`);
   `sealSurfaceSignatureArtifactsV513(projectId, runId, surfaceResult)` — каждая ожидаемая сигнатура есть в staging и равна.
2. **Возможности v517** 134320: на каждый мод surface `modAnalysisState = {...promoted, ...item, artifactRoute, …,
   physicalPathIndexComplete (scan → true, иначе как было), artifactCapabilities = OR(прошлые, текущие)}`; перечитать и проверить:
   state есть, индекс полный, все `missingCapabilities` получены, ни одна прошлая не потеряна (`lost-capability`). «Текущие» = `true`
   по конфигу (см. §5).
3. **Реестр v507** и **маршруты v518** (§4.3).
4. **Свидетель** `createCutoverRuntimeWitnessV503` 132932: `ready = barrierProof.finalCommitReady && final.openTokens===0 &&
   telemetry.status==='complete' && issues.errors===0`.
5. **Ядро** `canCommitReady` (CP3 168260): не failed/cancelled, нет очередных/активных источников, пусто в наблюдениях, нет
   очередной/активной работы, пусто в route-writes, нет открытых токенов.
6. **Архивы**: `archiveScanEnabled !== false ∧ archiveSourcesFailed > 0` → throw (134300).
7. **Конфликты** — §4.5. 8. **Сводка метрик** — печать v639 в `applyCutoverAggregateProjectionV503` (§4.8) — уже **после** promote.
- **Приёмка V522** 177543 (после терминала, не блокирует): `routeAcceptanceSeal.passed===true ∧ issues=0`, есть
  `capabilityArtifactSeal` без ошибок, `surfaceSignatureArtifactSeal.sealed===true`; reuse|indexed-записи: `storage==='chunks'`,
  `pathStorageMode==='idb-chunks'`, `pathStorageHotCopy===true`, нет `pathStorageColdReady/DualWrite/coldSegmentRef`,
  `coldSegmentStatus ∈ {none, ∅}`, `pathCount>0 ⇒ chunkCount>0`; 18 проверок, среди них «≥2 разных runtime-проекции» и
  `max(workRevisions) ≥ 2`.

### 4.5. Конфликты в живом пути
- Авторитетный финализатор (S13): `rebuildOverwriteOwnersFromPaths` 120968 — владельцы из `buildProjectModLookup()` 142019
  (обход разделителей, пропуск **только SYNC**, priority с 1 по порядку modlist, `enabled: !!mod.enabled`); map по модам
  (`getModPaths`, `isConflictPathIgnored` 143212, словарь путей), 256 партиций по FNV-1a(pathId), reduce по партиции: конфликт —
  ≥2 владельца, одновладельческие пути удаляются из `conflictPathDict`; сводки модов `conflictPaths, wins, losses, samplePaths≤12,
  opponentRefs, opponentNames≤32`. Затем `rebuildOverwriteChainsAndContextsFromOwners` 120780: mod-files → exact chains
  (`exact-stack-partition-reduce`) → `assignAuthoritativeExactChainIdentities` → route groups → contexts (+read-views).
- `conflictParity` (schema `…authoritative-conflict-validation.v2`): `singleTruth`, `ownerFinalizerComplete`, `chainsFinalized`,
  `contextMappingsComplete` (mappingsWritten = chains, missing 0, viewsMissing 0, staleChainIdsSkipped 0), `contextCardinalityComplete`
  (views = contexts; `chains=0 ⇒ contexts=0`, иначе `0 < contexts ≤ chains`), `streamingAlternativeDisabled` (CP10 дал нули).
- **`collectCutoverOwnerMetaV503` в живом пути не влияет ни на что** [пров. П2]: мета передаётся только в CP10, который выключен;
  IDB-финализатор берёт приоритет сам из `buildProjectModLookup`. Нюанс S15 про `getLiveModOverwritePriority` (локальна в 76014)
  касается только мёртвой ветки.
- Алгоритм CP10 (выключен; эталон для `sat-core::overwrite`, S20 §4): порядок владельцев `priority` ↑, затем имя; победитель —
  последний **включённый**; конфликт — ≥2 владельца (loose + virtual вместе); цепочка — группа путей с одной подписью
  `route:<все|||>###active:<вкл.>###winner:<имя>`; зубы `W/L/O/M`; точные маршрутные группы по `routeSignature`; производные
  жадно `subroute-absorption → common-tail ≥2 → shared-route-edge ≥1` при одинаковом победителе; контексты — union-find по общим модам.

### 4.6. Анализаторы обхода
- **CP7 обход** 170953: моды последовательно; итеративный DFS (`maxDepth` 64, каталоги перед файлами, `localeCompare base`), `.bsa`-каталоги
  **не** пропускаются (в отличие от surface); `archiveSourceId = route-source:${kind}:${hash(projectId\0mod\0path)}`; observation id
  `${runId}:loose:${hash}`. Архивы любой глубины → source-expander (`archive-bsa|archive-ba2`, токен барьера), чтение
  `ModlistToolArchiveEntrySizeAuthorityV555.normalizeEntries(await archiveReader(file))`; виртуальные наблюдения `size = entry.byteLength`,
  `modifiedAt = mtime архива`. Сбой архива внутри CP7 — деградация (`failSource`, токен закрывается со `status:'error'`), но cutover
  потом валит весь пробег (§4.4 п.6). Архивный файл даёт и loose-маршрут `archive-container`, и виртуальные `archive-entry`.
- **Слияние метаданных пути внутри мода** (`mergeSinkRouteMetadataV554` 169794): loose побеждает архив (`sourceCount` суммируется,
  `shadowedSourceCount+1`); тот же sourceId+archivePath → прежнее; два разных архива → `archive-ambiguous`, `byteLength null`,
  `metadataStatus 'ambiguous-multiple-archives'`; иначе `ambiguous` (`ambiguous-multiple-sources`, совпадающие поля сохраняются).
- `routeMetadataSummaryByMod` 171028: `effectiveContentFileCount = contentFiles + archiveEntries` (контейнеры исключены),
  `effectiveContentByteCoveragePercent = round(known·10000/count)/100` (100 при 0), `archiveEntryBytes = null` при неполном покрытии.
- **CP8 агрегаты** (S20 §2/§4): physical = `origin loose ∧ kind loose-file`, virtual = `origin archive ∧ kind virtual-file`;
  `physicalBytes` только physical, неизвестный размер → `physicalSizeUnknownCount`; `routeCount` = все наблюдения; `folders` = все
  промежуточные сегменты (кап 128), `root` — первый сегмент при глубине >1; кап расширений 128 → `(other)`; точные счётчики v602 без капа.
  Кандидаты: plugin/archive/dll — только physical; config (`.ini .json .toml .yaml .yml .xml`) — любое происхождение.
  Теги: Σ`score` (по умолчанию 5) сработавших детекторов, порог **≥5**, сорт по счёту ↓, затем имя; фильтр по именам/`short` коллекций
  кроме `stickers` (если коллекции есть). Детектор: группы по полям `path, file, plugin, name, folder, root, ext` + `all[]` + `none[]`;
  `folder/root/ext` — точное равенство (lower), остальные — подстрока или шаблон `*` без якорей; совпадение = (нет позитивов ∨ хоть одно)
  ∧ все `all` ∧ ни одного `none`. Стикеры: 🛠️ `.dll` ∨ папка `skse` ∨ `skse/plugins`; 📦 esp/esm/esl; 📌 `interface` ∨ `.swf`;
  🎯 `animations`/`behaviors` ∨ `.hkx`; 🧪 подстрока `test`/`debug`; 💋 папка `textures` ∧ `actors/character`; 🐺 подстроки
  `dragon/draugr/falmer/creature/enemy`; фильтр по коллекции `stickers` без нормализации.
- **CP9 плагины**: только `loose-file`/`loose`, ext esm/esp/esl, `provenance.rootLevel ∧ нет '/'`; парсер — глобальный
  `parsePluginHeaderFromFile` 100666 (TES4: ESM `0x1`, ESL `0x200`; `MAST/CNAM/SNAM/HEDR(f32,u32,u32)`, `XXXX`; скан записей v611
  22792: 24-байтные заголовки, стек GRUP, `TES4` не считается, ссылки = REFR+ACHR, статусы `complete|partial|unavailable|error`);
  тип `resolvePluginType`: ESL-флаг → `.esp`?`espfe`:`esl`; иначе по расширению; иначе ESM-флаг → `esm`; иначе `unknown`.
  Дедуп по нормализованному пути (первый выигрывает); сбой → запись `recordScanStatus 'error'`, пробег продолжается.
- **CP15 DDS VRAM** (102495–103310, S11): accepts `.dds|.bsa|.ba2`, MANDATORY. Loose DDS: читаются 148 байт; проверки `'DDS '`,
  `u32@4==124`, `u32@76==32`; `height@12, width@16, depth@24, mips@28, FourCC@84, caps2@112` (volume `0x200000`, cubemap `0x200`,
  грани `0xFC00`, нет граней → 6); DX10: `dxgi@128, resourceDimension@132` (3 → 1D, 5 → volume), `misc@136 & 0x4` → 6 граней, `arraySize@140`.
  Формула: Σ по `l < mips`: `w=max(1,w0>>l)`, `h=max(1,h0>>l)`, `d = volume ? max(1,d0>>l) : d0`; block `ceil(w/4)·ceil(h/4)·blockBytes·d`;
  linear `w·h·Bpp·d`; linear-bits `ceil(w·bits/8)·h·d`; × layers × faces; не safe-integer → null. Таблицы блоков: BC1 70–72, BC4 79–81,
  DXT1/ATI1/BC4U/BC4S → 8; BC2/3/5/6H/7 (73–78, 82–84, 94–99), DXT2–5/ATI2/BC5U/BC5S → 16; Bpp: 2→16, 6→12, 10/16→8, 24–43→4,
  49–59→2, 61–65→1, 87–93→4; legacy без FourCC и `rgbBitCount@88 ∈ (0,128]` → linear-bits. Архивы: VRAM известен **только для BA2 DX10**
  = `texturePayloadBytes` (Σunpacked чанков, без заголовка DDS 128/148); DDS в BSA и BA2-GNRL → `unavailable`. Статус мода: архивы
  выключены и есть контейнеры (`hiddenArchiveRisk`) → `partial` при known>0, иначе `unavailable`, coverage null; иначе `complete`
  (нет DDS или нет неизвестных) / `partial` / `unavailable`. Корзины `≤1K, 2K, 4K, 8K, >8K` по max(w,h).
- **Архивы BSA/BA2** (S11 §2): BSA v103/104/105, флаги `0x1/0x2` обязательны, `0x4` сжатие по умолчанию, бит размера `0x40000000`
  инвертирует; сжатый — u32-префикс распакованного размера; BA2 GNRL/DX10, размер DX10 = Σunpacked + 148 (DXGI {72,75,78,98,99}) или
  128; дедуп путей `ambiguous-duplicate-entry`; `normalizeArchiveVirtualPath` (lower, версия 4); лимит 2 000 000 записей.

### 4.7. Проекция метрик `applyCutoverAggregateProjectionV503` 133004 (текст читают самотесты v610/v612)
1. `fullReuseNames` = строки `FULL_REUSE/reuse`. 2. `carryForwardFullReuseMetricSummariesV612` 103515 — **всё-или-ничего**:
   фаза 1 валидирует все моды без мутации (пропуск `pending|error`, контракт v618); любой missing/invalid → ничего; фаза 2 — рабочая
   копия, `status='active'`, `revision+1`, `digest=''`, пометка причины (≤768, `buildMetricReuseReasonV635`). `atomicCommitted !== true` → throw.
3. reuse-мод без агрегата: обязателен `copiedByModName[mod] > 0`; пишутся только `commitConflictMetricSummariesV645` и
   `commitRuntimeRouteMetricSummariesV645` (свежие). 4. scan/indexed-мод: агрегат и `mod.id` обязательны; `mod.derived` (§3.8);
   `commitAllAvailableModMetricsV610` 102404 с `coreValues{sizeBytes = physicalBytes ?? round(MB·1024²), fileCount=routeCount, dirCount}`,
   `aggregate, ddsVram, pluginScan, archiveScan, conflictFacts, runtimeRoute, surfaceState, routeMetadata`; `sourceRevision` =
   сигнатура || digest; теги/стикеры (если правила вкл.) → `applyTagNamesToMod` 157432 / `applyStickerNamesToMod` 157443.
5. Overwrite: та же материализация с `runtimeRoute{executionRoute:'scan', routeKind:'FULL_SCAN', reason:'system-overwrite-folder'}`.
6. `finalizeAnalysisMetricSummaryV599(project, runRef, {status:'done', expectedEntityKeys})`; нет digest → throw (после promote!).
- Порядок провайдеров в `commitAllAvailableModMetricsV610`: core v599 → aggregate (v645 raw facts, v600 inventory, v601 runtime assets,
  v603 cargo → v604…v609) → ddsVram v613 → pluginScan (v600, v645 структурные, v611 записи) → archiveScan v645 → surface/routeMetadata v647
  → conflictFacts v645 → runtimeRoute v645. Правило v642: нет результата провайдера = нет метрики (без выдуманных `unavailable`).
- Формулы v645: `modPhysicalSizeCoveragePercent = round2((physical−unknown)/physical·100)` (100 при 0);
  `modArchiveFileSharePercent = round2(virtual/total·100)`, total = `routeCount || physical+virtual`; `modConflictWinRatePercent =
  round2(wins/conflictPaths·100)`; route: `scan|indexed|reuse → FULL_SCAN|INDEXED_ANALYSIS|FULL_REUSE`. Неполные покрытия → «нижняя
  граница», `confidence = coverage/100`. Остальные экстракторы (v602–v609) — S01 §4 (таблицы порогов, переносить таблицами).

### 4.8. Сводка метрик: жизненный цикл, печать, ворота (S01)
- Авторитет `resolveProjectAnalysisRunAuthorityV616`: кандидаты `treeFull.analysisStore.{projectId,runId}`, `treeFull.{projectId,runId}`,
  `treeFull.projectId + analysisGeneration.authoritativeRunId`, `meta.analysisProjectId + meta.analysisGeneration.authoritativeRunId`;
  ровно одна уникальная полная пара → resolved, >1 → conflict, 0 → missing; гибрид половин запрещён.
- `ensure…Generation` 21045: смена поколения **или** то же поколение в `idle/stale/error/cancelled` → чистое ведро (rev 0 / +1),
  status `active`; `active`/`done` того же поколения не трогает. `commit` 21501: status `active`, rev+1, digest='' (контракт проверяется
  после записи и не блокирует). `finalize` 22230: только из `active` (idempotent `done`); ворота `evaluate…SealEligibilityV639` 22129 —
  ref совпадает, аудит контракта, полнота `expectedEntityKeys × requiredMetricIds` (нет сущности/метрики/`pending|error` → отказ;
  `unavailable/unsupported` проходят); успех → `done`, `terminal=null`, digest; отказ → `error` + `terminal{phase:'metric-summary-seal-admission-v639'}`.
  `fail` v636 / `cancel` v637 — только из `active` того же поколения, частичные записи сохраняются, rev+1.
- Digest 21107: FNV-1a 32 по `JSON.stringify({schema, projectId, runId, status, revision, dictionaries, byEntityKey})` канонического
  ведра (ключи сущностей/метрик/map отсортированы `localeCompare`) → `metrics-v2-%08x`.
- Чтение 21533, порядок ворот: integrity → authority → generation (`stale`) → `error` → `cancelled` → отсутствие (active → pending;
  error → error; done → `unavailable` с `canonicalAbsence:true`) → контракт. Legacy-резолвер — только если `runs.metrics` нет вовсе.
- **Ворота FULL_REUSE v632** 103346: `runs.metrics` есть, `bucket.runId === previousRunId`, `status==='done'` (иначе
  `metric-summary-not-sealed:<status>`), целостность v615 и контракт v618 валидны; сущность мода имеет **все** метрики
  `getRegisteredMetrics({scope:'mod', availability:'analysis'})` и ни одной `pending|error`.
- Допуск при импорте v628 — таблица S01 §4 п.7 (active → stale/quarantined-interrupted; done с битым digest → error; …).

### 4.9. Лимиты и прогресс
- Cutover → CP11/CP7: `observationWindow 64, routeChunkSize 128, maxPendingObservations 128, maxBufferedRouteWrites 256,
  maxPendingWorkUnits 128, maxActiveArchiveReaders 1, maxEventHistory 256, yieldEveryNItems 128, pathSamplesPerMod 24,
  candidateSamplesPerKind 16`. Дефолты ядра CP3: 4/2/2048/1024/512/1000/128; CP7: maxActiveSources 2, readers 1, 256/256/64/768.
  Обратное давление CP3 — **исключение** при переполнении (вызывающие сами дренируют до порога).
- CP11 проекция `run-session-projection.v1`: `attached, status, runId, phase, revision, sources{total, complete, failed,
  byKind{kind:{total,queued,active,complete,failed}}, recent}, queues{…, highWaterMarks}, analyzers{count, contracts, recent{consumed,
  degraded}}, barriers{…, recent{openedTokens, closedTokens}}, counters, throughput (мгновенные Δ/Δt), issues{warnings, errors,
  recent[8]}, events{total, retained, dropped, recent[16]}`; троттлинг 32 мс; кольцо 160 событий.
- Шкала deep окна (S07, `buildRunSessionDeepMasterV503` 67569): фазы traversal 0–70, drain 70–85, post-deep 85–97, promotion 97–100;
  traversal `70·complete/total`; drain `70 + 15·min(obsTotal, consumed(cp08), consumed(cp10))/obsTotal`; post-deep `85 + 12·postRatio`;
  монотонно в пределах `uiSessionId`; проценты **усекаются** до десятых. 12 плиток `loose|archives|virtual|plugins|metrics|tags|
  stickers|owners|chains|contexts|readViews|promotion`. Предварительный SYNC: `found=foldersSeen`, `synchronized=found−missingInProject`,
  `unsynchronized=missingInProject+missingOnDisk`.
- Стадии UI → шаги маршрута (V520 69129): sync→discovery, surface→surface, deep/plugins/metrics/tags/stickers→deep, archives→queue-drain,
  compact/overwrite/overwrite-folder/finalize→post-deep.

### 4.10. Хеши и идентификаторы
- `hashStringToInt` 136021 / CP1 / сводка метрик / owners-partition: FNV-1a 32 (2166136261, 16777619, `>>>0`).
- CP6/CP7/CP8/CP9 `hashTextV503`: h1 FNV-1a 32, `h2 = imul(h2^c, 2246822507)+i` → `${h1_36}_${h2_36}`. CP10/CP11 — другой
  (h2 seed `0x9e3779b9`, `imul(h2, 0x85ebca6b)`); CP12 — третий. CP5 sourceId `route-source:${kind}:${slug[0..48]}:${fnv8hex}`;
  CP7 мод `route-source:mod-folder:${hash(projectId\0name)}` (форматы CP5 и CP7 разные).

## 5. Расхождения и вероятные ошибки

Уровни: **[код]** — подтверждено чтением кода; **[вер.]** — вероятно; **[н/пр.]** — не проверено.

1. **Переназначения `window.X` не перехватывают вызовы внутри IIFE** — [код, П2] (19456/166057, xref depth=1, вызовы 35963/36424/
   36463/37522/107695/108139). Опровергает S15/S20/S21 («меняет голый идентификатор во всех скриптах»). Для области 5: SYNC
   precommit запускает surface-пробег мимо CP4/Run Store; CP4 видит только запуск из `runAnalysisPipeline`.
2. **Один упавший BSA/BA2 валит всю «Бездну»** при включённом анализе архивов — [код, П2] 134300. CP7 задуман как деградация,
   CP1-граница требует «no archive failure»; итог — строгий отказ и abort поколения.
3. **Возможности v517 ставятся по конфигу, а не по факту работы анализатора** — [код] S15 134326–134346; печать сверяет с тем же объектом.
4. **Пути staging в нижнем регистре** (CP7/CP8 вне IIFE не видят `normalizeAnalysisPath` 113557 — в `window` не экспортирован,
   [код, П2 grep]); legacy-поколения — с регистром. Конфликты не страдают (`normalizeTreeConflictPath` тоже lower), но паритет
   путей с legacy и показ путей — да — [вер.] S19.
5. **Мета владельцев cutover мертва**: `collectCutoverOwnerMetaV503` кормит только выключенный CP10; IDB-финализатор берёт порядок из
   `buildProjectModLookup` — [код, П2]. Ошибка S15 про `getLiveModOverwritePriority` неактуальна. Зато `buildProjectModLookup` пропускает
   только SYNC и `enabled: !!mod.enabled`, а живой оверлей — все функциональные и `!== false` — [код] S13.
6. **Сверка переименований по сигнатуре не работает**: сигнатура содержит имя папки; в «Бездне» ещё и пустой surface
   (`buildSyncReconcileResult(folderNames, {})` 134083) — [код] S15, П2. Подтверждает подозрение 00-overview / NG-D002.
7. **Surface не берётся из единого обхода** (`buildCutoverSurfaceResultV503` не вызывается) — [код] S15. **[испр.]** 00-overview п.2/5.
8. **`indexed` пересчитывает все анализаторы по всем путям мода**, а не только недостающую возможность (обход идёт целиком через
   поддельный handle; экономится только перечисление каталогов) — [вер.] по S15/S19 (явного фильтра анализаторов по `missingCapabilities`
   в cutover не описано). **[испр.]** 00-overview «пересчитывается только его возможность».
9. **Печать сводки метрик и проекции — после promote**: сбой `finalizeAnalysisMetricSummaryV599` бросает уже после смены авторитета
   (`promoted=true` → abort не делается, catch пишет `failAnalysisMetricSummaryV636`) — [вер.] S15 п.13 + S01.
10. **custom-стадия недостижима из UI**: `customArmed` нет кнопки, по умолчанию false — [код] S12; `runEnabledCustomRuns` 92909 — `new Function`.
11. **CP5 RouteSourceScheduler `runLegacyShadow` без флага** на каждом surface-пробеге (лишняя работа, только лог) — [код, П2] 132433.
12. **CP6 `consume` ссылается на необъявленный `ARCHIVE_EXTENSIONS_V503`** → самотест CP6 почти наверняка `ok:false`; живой путь не
   задет (у CP7 своя копия) — [вер.] S19.
13. **CP6 `surfaceSignatureV503`** — fallback `surface_shadow_v1_…` (buildAnalysisSignature вне области) — в живом пути не используется — [код] S19.
14. **Статусы `error` vs `failed`** (словарь CP2 vs ядро CP3) — [код] S19.
15. **IDB O(всё)**: `listStagedModPathRecordsV507` — курсор по всему `modPaths`; `cleanup` при abort — по всем stores; V522 — полный
   курсор по `modPaths` всех проектов после каждого пробега — [код] S15/S19/S21.
16. **reuse = физическое копирование путей** между поколениями — [код] S15 133659.
17. **V522 мутирует завершённый пробег** (самопереход `completed→completed`, `errors += failures`) — [код] S21.
18. **Мусор самотестов CP3 в Run Store навсегда**; фильтр — подстрока `self-test|compatibility` — [код] S21.
19. **Усиление записей**: 13 коммитов в Store на одну проекцию барьеров + полный `innerHTML` техстраницы; `renderRunSession` пересобирает
   адаптер на каждый кадр — [код] S07/S21; вероятный источник лагов окна.
20. **Один глобальный слот `__modlistToolAuthoritativeAnalysisActiveRunV520`** без защиты от параллельных пробегов — [код] S21
   (с учётом п.1 вложенный precommit-пробег его не трогает).
21. **`authTransition`** — `Object.assign(record, patch)` без белого списка — [код] S21.
22. **`commit` в запечатанное ведро** снимает печать молча; контракт не блокирует запись — [код] S01.
23. **`finalize` без `expectedEntityKeys`** на 47285/47386 (LG) — ворота полноты пусты — [н/пр.] S01.
24. **`runUnifiedModMetricsStageSelfTestV610`** требует строку `requiredMetricIds` в тексте legacy-стадии — вероятно `ok:false` — [вер.] S11.
25. **Самотест v626** — ReferenceError `ANALYSIS_METRIC_SUMMARY_VERSION_V599` — [код] S01.
26. **Самотесты по исходному тексту** (v508, v509, v515, v610, v612, v632, CP1 `validateCP1`, V520) — хрупкие «линтеры» — [код].
27. **Комментарии-шапки recovery врут** («shadow», «never executes work»), номера CP шапки ≠ id скрипта — [код] S21.
28. **CP8**: config-кандидаты из архивов (несимметрично); 🧪/🐺 по подстроке (`latest`, `contest`…) — ложные стикеры; эмодзи без
   нормализации (`U+FE0F`) — [код] S20. Обнаруженные стикеры оседают в `mod.stickers` (ручные) и не снимаются (157443) — [код] S15.
29. **CP9**: `name` записи-сбоя в lower-case, у нормальных — исходный — [код] S20.
30. **CP10** (выключен): `stagedChunks` хранит весь индекс; участники O(F²·P); `exactRouteGroups.activeWinnerModName` от первой цепочки — [код] S20.
31. **UI**: `renderHeaderPreviews` 135899 в IDB-режиме показывает «0 модов» для «Бездны»; `runAnalysisPipeline` требует overwrite при
   `deepEnabled===undefined`, а режим тогда `surface`; `cancelled → warning` в окне; `#runSessionRuntime` не скроллится (CP7-стили) вопреки
   CP12-пейджеру; окно следует только за `analysis/intake` — [код] S15/S07/S21.
32. **Overwrite-мета не получает `failed`**: при исключении `overwriteMeta.status` остаётся `running` — [код] S13.
33. **Ручной перезапуск конфликтов** строит цепочки дважды (154290) — [код] S11.
34. **Дубли имён модов**: `findProjectModByNameV612` — первый по точному имени; `applyTagNamesToMod` — по `normalizeName` — [код] S11.

## 6. Что берёт NG и что меняет (к `docs/distances/NG-D002-run-pipeline.md`)

| Монолит | NG (сейчас) | Решение |
|---|---|---|
| Вход: кнопка → CP4 intake → cutover; SYNC precommit — surface мимо ядра | `sat-scan::scan(jobs)` | Один вход «пробег» как задача (NG-D003 jobs), режимы `surface`/`full` с тем же смыслом; precommit SYNC запускает **ту же** задачу surface (исправление п.5.1) |
| Run Contract (12 статусов, таблица переходов), Run Store | нет | Перенести таблицу переходов как enum + `fn can_transition`; патч только по белому списку; один runId задачи = runId поколения (у монолита два разных) |
| 13 барьеров ядра recovery | нет | Для анализа нужны только `SOURCE_IDENTITY` (папки выбраны), `CAPABILITY_AVAILABLE`, `WORK_DRAINED`, `STAGING_COMPLETE`; остальные — SYNC (NG-D003) |
| Словарь CP2 (`RouteSource / Observation / WorkUnit / BarrierToken / RunEvent / RuntimeSnapshot`) | `ScanJob`, `LooseFile`, `ArchiveEntry` в `sat-scan`/`sat-formats` | `RouteSource` = `ScanJob` + архив как дочерний источник; `Observation` = запись обхода; события — канал задачи |
| Контракт анализатора (`executionClass`, `failurePolicy`, `accepts/consume/flush/finalize`, порядок регистрации = порядок диспетчеризации) | разбор форматов вшит в обход | trait `Analyzer` в `sat-core`/`sat-scan`, встроенные регистрируются как будущие расширения (NG-D002 п.1a); один enum статусов (без `error`/`failed`) |
| Обратное давление исключением, окна 64/128… | rayon I/O-пул | Не копировать; ограничение — пул и ограниченные каналы, параметры по замерам |
| Барьер `routes.complete` (physical-root, storage-flush, токен на архив) | join после обхода | Явная join-точка перед финализаторами; счётчик ожидаемых архивов |
| Сбой архива = отказ всего пробега (134300) | — | **Меняем:** деградация на мод (архив `error`, метрики мода `partial`), пробег завершается; в отчёт — список битых архивов |
| Маршруты `scan/indexed/reuse` по `artifactCapabilities` v7, blocked до deep | scan/reuse по `CACHE_VERSION` (`sat-scan/src/cache.rs`) | Версия **каждого анализатора** в записи кеша мода; `indexed` = прошлый индекс путей + запуск **только** анализаторов с устаревшей версией (исправить п.8); возможность ставится по факту результата анализатора (исправить п.3); нечитаемый артефакт `unchanged`-мода → `scan`, а не блок (обсудить: монолит блокирует) |
| Поколение `staging → authoritative \| aborted`, оптимистическая проверка предка, SYNC неприкосновенен, откат указателя | нет | Снимок результата в памяти + атомарная замена `Arc`; проверка «предок не сменился»; abort = drop staging (без O(БД) cleanup) |
| reuse = копирование путей | кеш по моду | reuse = ссылка на снимок мода |
| Печати v507/v513/v517/v518, свидетель, конфликтные инварианты | нет | Инварианты перед публикацией (`debug_assert` + ошибка задачи): staging = ровно текущие моды + Overwrite; у каждого мода полный индекс; ни одна прошлая возможность не потеряна; reuse не в обходе; сигнатуры совпадают; `chains=0⇒contexts=0`, `contexts≤chains`, маппинги полны; очереди пусты, ошибок 0. Печать сводки метрик — **до** публикации |
| Сигнатура `имя|count|dirs|files|exts` | v2 (глубина 2, размеры, mtime, без имени) | Сознательное расхождение (NG-D002 «Решения»); фикстура `Alpha|4|…` — в тест паритета режима совместимости, если нужен импорт |
| CP7 обход, архивы любой глубины, `archive-container` + `archive-entry`, слияние v554 | обход + `sat-formats::archive` | Перенести роли маршрутов и правило слияния v554 в `sat-core::facts`; сверить размеры записей с V555 (BSA u32-префикс, DX10 Σunpacked+128/148) |
| CP8 агрегаты, теги ≥5, стикеры | `ModMetrics` (`sat-core/src/facts.rs`: bytes, files, dirs, dds, nif, pex, hkx, dll, plugins, archives, archive_files…) | Добавить `physical/virtual/route` счётчики, `physicalSizeUnknown`, точные семейства v602 (9 семейств), топологию v607; капы (128, 24…) не нужны. Стикеры 🧪/🐺 — по сегментам пути, не подстроке (исправить п.28); config-кандидаты — только physical |
| CP9 плагины (корневые, TES4, скан записей v611) | `sat-formats::plugin` — только заголовки | Добавить скан записей (GRUP-стек, REFR+ACHR, статусы); тип `resolvePluginType` дословно |
| CP15 DDS VRAM в том же обходе | есть (`sat-formats::dds`) | Сверить формулу/таблицы §4.6 и статус `hiddenArchiveRisk`; VRAM из архивов только BA2 DX10 |
| Конфликты: авторитетный IDB-финализатор (owners → exact chains → route groups → contexts); CP10 выключен | `sat-core::overwrite` (стеки, цепочки, контексты) | Эталон для паритета — **авторитетный финализатор** (120780/120968), CP10 — справочник алгоритма; порядок владельцев — живой порядок modlist, единое определение «пропускаемых разделителей» и `enabled` (исправить п.5) |
| Сводка метрик v599 (кортежи, словари, digest FNV, ворота чтения/печати, carry-forward всё-или-ничего) | нет | Типизированная таблица метрик в снимке; печать с воротами полноты `сущности × обязательные метрики`; `unavailable` — честное отсутствие; carry-forward reuse атомарно; контракт значения проверять при записи (исправить п.22). Словарное кодирование — только для импорта `.mnlth` (ADR 0007) |
| После promote: проекции, нормализованный слой v660, custom (`new Function`), LG lifecycle, cold migration | нет | Нормализованный слой — отдельная стадия до публикации (область 10); custom — через песочницу расширений (GOLD «не переносим»); cold migration и compact — не переносятся (NG-D002 «Решения»); LG — Фаза 3 |
| CP11 проекция окна пробега, шкала deep, 12 плиток | — | Контракт `RunSnapshot` для NG-D003/D004: формы §4.9; шкала — для паритета UI |
| Самотесты по тексту функций, CP1 отпечатки | — | Не переносить; инварианты §4.4 — в тесты Rust |

## 7. Открытые вопросы

1. Кто вызывает `window.runSyncPrecommitCheck` / `window.executeSyncCommitPlan` (CP5) после поправки п.1? Если никто из UI — SYNC issuance
   CP5 и donor CP6 фактически консольные; пересмотреть области 11/12 и S21 §3.2.
2. Точное тело печати v517 (134320–134395): какие ключи `currentCapabilities` и откуда — проверить, что действительно `true` по конфигу для reuse/indexed.
3. Фильтрует ли CP7/CP8 работу для `indexed`-модов по `missingCapabilities` (п.8) — прочитать `createIndexedDirectorySourceV516` + регистрацию анализаторов.
4. Что делает catch cutover, если исключение после promote (`promoted=true`): остаётся ли сессия `failed`, а поколение авторитетным с неполной
   сводкой метрик (`error`)? Как это видит следующий пробег (ворота v632 → все `indexed` из-за `metrics`)?
5. `buildProjectModLookup` включает ли системный мод Overwrite и с каким приоритетом (в cutover-мете было `highest + 1 000 000`)?
6. Результаты самотестов при загрузке в браузере: CP06 (ожидаю `ok:false`), V610 unified metrics, v626, CP1 baseline.
7. 47285/47386/49935 — живой путь LG или самотесты (печать без ворот полноты)?
8. `commitSurfaceResults` 154234 / `commitDeepResults` 154329 — точные поля после promote (в конспект области 6).
9. Регистр путей в reuse-поколениях после legacy-поколения: смешение регистров в одном `modPaths` при переходе — возможно ли.
10. Параметры по замерам для NG (нагрузка 7 525 модов): сколько `indexed` на типичном повторном пробеге и сколько стоит перечисление каталогов против чтения содержимого.

Память пользователя в этом конспекте не использовалась.
