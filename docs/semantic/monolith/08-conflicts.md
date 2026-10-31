# 08 — Хранилище анализа и конфликты перезаписи (области 6, 8, 10)

> Проход 2 (сшивка). Источники: `pass/S12`, `S13`, `S14` (хранилище, конвейер, нормализованный слой), `S09`, `S08` (проекция, зубы, дерево, бейджи),
> `S15`, `S16`, `S19` (движок идентичности, граф связей, RAM-наследие, советник), `S17` (presentation-слой модалки), `S05` (LG-стор), `S11`, `S20` (вызовы из пробега).
> Номера строк — монолит `index.html`. «Живое» — последнее присваивание по порядку исполнения.

## 1. Коротко

- Конфликт = нормализованный путь (lower-case), у которого **≥2 владельцев** среди модов проекта (включённых или нет). Победитель пути = **последний
  включённый** владелец по возрастанию приоритета (приоритет = порядковый номер мода в modlist сверху вниз; ниже — сильнее, как в MO2).
- Живой конвейер (IDB, `AnalysisPathStore`, БД `modlisttool-analysis-paths-v1` v15, 35 stores) — внешняя сортировка через 256 партиций:
  `owners → mod-files → exact chains → identities → route groups → contexts (+read-views)`, затем после promote — **нормализованный слой v501**
  (материализация v501 → миграция идентичности v502 → граница поколения v660).
- **Точная цепочка = один стек**: все пути с одинаковым упорядоченным набором владельцев, набором включённых и победителем. Точный слой неизменяем;
  route groups и нормализованные цепочки — производные.
- **Контекст** = компонента связности «цепочки, делящие мод» (label propagation по min-label). Имя контекста наследуется при score ≥ 0,28.
- Сохранённые priority/enabled — снимок; при каждом чтении накладывается **живой оверлей** из `state.project` (v239), поэтому перестановка модов
  не требует перепробега — но ломает «свежесть» v499/v500 (дайджест порядка v658) и лёгкие превью.
- Модалка конфликтов показывает **presentation-сущности** (v654/v661): safe-группы v499 из нормализованных цепочек, пользовательские композиты v657;
  Done (v665) и ручные имена (v666) ключуются **отпечатком членства в атомах истины** (`sourceTruthFingerprints`), а не id цепочки.
- Над всем этим — авторитет логических групп (LG v565/v571): если в проекте есть группы, чтение цепочек идёт из LG-стора, а не из IDB/RAM.
- NG (`sat-core::overwrite`) уже имеет стеки/цепочки/контексты/W-L-M-X как чистую функцию `(table, priorities)`; нет идентичности, route groups,
  нормализованного слоя, игнор-листа, Done/имён. Есть **две семантические разницы**: loose>archive и выключенные моды (см. §5).

## 2. Живой путь (от пробега до модалки, дерева и бейджей)

1. **Пути модов.** Глубинный анализ пишет пути через `PathStorage.writeModPaths` (126 913+) → `AnalysisPathStore.putModPaths` (114 284):
   `normalizeAnalysisPaths` (113 557, регистр сохраняется) → контракт горячей записи `normalizeHotPathStorageWriteExtraV519` (113 998) →
   чанки по 1000 в `modPathChunks` + метаданные маршрутов v554 (`modRouteMetadata*`, чанк 512) одной транзакцией `writeModPathsBundleV554` (114 217).
   Архивы добавляются `appendModPaths` (114 481) со слиянием `mergeRouteMetadataEntriesV554` (loose > archive **внутри одного мода**).
   После deep — `runPostDeepAutoColdMigration` (128 482, ← 132 753 / 134 541) может увести сырые пути в холодный `.mnlth` (`putModPathsManifestOnly`).
2. **Чтение путей** — `getModPaths` (114 745): warm/LRU → запись `modPaths` → холодная: hot-recovery v519 (`readHotModPathChunksV519` 114 618) или
   `readColdModPathsFromMnlth` (114 534, проверка хэша `hashAnalysisPathList` и числа путей) → горячие чанки → inline `paths`. Ошибка ⇒ `[]`.
3. **Стадия owners** — `rebuildOverwriteOwnersFromPaths` (120 968; ← cutover 134 415, legacy `runOverwriteOwnersStage` 104 299).
   Владельцы из `buildProjectModLookup` (142 019). Map: по модам `getModPaths` → `normalizeTreeConflictPath` (143 074) → `isConflictPathIgnored` (143 212)
   → `conflictPathId` → вклад в партицию `fnv(pathId)%256`. Reduce по партиции: одновладельческие пути удаляются из `conflictPathDict`,
   конфликтные пишутся в `overwritePathOwners` (v2-ключ `p::r::pid::<pathId>`), сводки модов `overwriteModSummary` сливаются между партициями.
4. **Оркестратор** `rebuildOverwriteChainsAndContextsFromOwners` (120 780; ← 134 416, 104 330, ручной 154 293):
   1. `buildOverwriteModFilesFromOwners` (116 744) → `overwriteModFiles` (партиции по `fnv(modName)`);
   2. `buildExactOverwriteChainsFromOwnersPartitioned` (120 141) → `overwriteChains` (lite v2), `overwriteChainByMod`, `overwriteChainPathChunks`;
   3. `assignAuthoritativeExactChainIdentities` (120 704) — все цепочки в RAM → `convertOverwriteChainToConflictGroup` (142 591) →
      `decorateConflictGroupsWithIdentityLegacyV476` (= исходная 138 133, захват 163 534) `{replace-source, repairGeneratedCollisions}` → имена обратно в IDB;
   4. `rebuildOverwriteRouteGroupsFromExactChains` (117 345) → `overwriteRouteGroups`, `overwriteRouteGroupByChain`;
   5. `buildOverwriteContextsFromChains` (119 231) → `overwriteContexts`, `overwriteContextByChain` + identity pass v468 + сразу
      `rebuildOverwriteReadViewsFromStoredGraph` (119 215) → `overwriteContextView`, `overwriteChainPreview`, `overwriteModView`.
5. **Проверка паритета в cutover** (134 414–134 453): `conflictParity` (mappings = chains, views = contexts, `0 < contexts ≤ chains`), иначе throw.
   Commit-патч кладёт в `runs.treeFull` только манифесты `overwriteOwnersStore`/`overwriteChainsStore`; `overwriteChains`/`conflictIndex` — пустые оболочки.
6. **После promote** (134 492–134 549; ошибки уже не откатывают поколение): `window.modlistToolMaterializeOverwriteNormalizedLayerV501({activate:true})` —
   живая обёртка **v660** (149 559) → v502 (126 781) → v501 `materializeV501` (125 286). v501: `runOverwriteRepeatCoverOptimizerV500` (123 903) поверх
   v498/v499 → гидратация точных → нормализованные цепочки/контексты → доказательство родословной → 6 stores `overwriteNormalized*` + превью v687 в
   `overwriteNormalizedMeta` (`page-preview::`). v502 `migrateAfterMaterialize` (126 661) переносит имена/Done/выделение. v660 сбрасывает runtime предложений.
7. **Cutover потребителей** (125 593–125 623): `AnalysisPathStore.getOverwriteChain / …ChainsForModFromStore / …ChainsPage / …ContextForChainFromStore /
   …Context` при `isReadyV501(p,r)` читают нормализованный слой, иначе `exactApiV501.*` (замкнутый в IIFE точный API).
8. **Источник цепочек для UI** — `getStoredOverwriteChains` (142 465), порядок: **(1) LG `getLogicalGroupNGAuthoritativeConflictStoreV571` (53 958)** →
   (2) `treeFull.overwriteNormalizedChains` (ready и непусто) → (3) `treeFull.overwriteChains` (legacy RAM, `buildOverwriteChainsFromTreeFull` 142 299) → пусто.
   Режим IDB определяется `isIndexedDbOverwriteMode` (143 257).
9. **Модалка** — `getConflictModalGroups` (144 396): LG v571 → IDB `idb.groups` (иначе запускает `loadConflictModalGroupsFromIdb` — живая 125 956,
   и возвращает `[]`, догрузка через `scheduleConflictIdbLazyRerender` 143 463) → RAM convert+decorate → legacy `ensureConflictCache`.
   `loadConflictModalGroupsFromIdb` v501/v687: лёгкие превью, если свежесть v658 и `lightPageReadReadyV687`, иначе `getChainsPageV501`; →
   `buildIdbConflictGroupsFromChains` (142 723: convert → hydrate → **`decorateConflictGroupsWithIdentityGraphV476`**, живая с 164 575) →
   `attachNormalizedContextsV501` (v690 batch) → `prepareConflictResidentModalPageV686` → `mergeIdbConflictGroupsIntoUiCache`.
10. **Рендер модалки** — `buildConflictsMarkup` (153 697): миграция legacy Done → V665; `groupConflictModalGroupsByContext` (144 543) →
    `projectConflictBucketsToV499SafeCardsV654` (151 692; карточки = safe-группы v499 нормализованной цепочки; идентичность — на полном наборе) →
    `reconcileOverwriteUserCompositesV657` → `collapseDurableConflictSafeGroupsV657` → поиск v693. `ui.cardsAuthorityEffectiveV671 =
    есть safe-группы ? 'presentation' : 'backend-fallback'`; `cardsProjectionStaleV658` = порядок изменён. Карточка — `renderConflictModalGroupCard` (151 835).
    Детали карточки лениво: `loadConflictPresentationEntityDetailsV663` (150 287) читает **точные** цепочки `sourceExactChainIds` (`hydrate:true`),
    при неполном покрытии → `null`. Таблица контекста — `buildOverwriteContextForChainAsync` (144 995) → IDB-вариант 144 809 (chainIds из сохранённого
    контекста) или RAM BFS 144 600; stack-view `buildOverwriteContextStacks` (148 222).
11. **Проекция одной цепочки** — `buildConflictProjectionForGroup` (86 292, контракт `conflict-projection-truth-v490`) кешируется в `group.conflictProjection`
    (`ensureConflictProjectionForGroup`, ключ `runtimeStamp`). «Зубной ряд» — `renderConflictChainParticipantBar` (85 604) + canvas v487 (86 731,
    `v538-flat-muted-teeth`), состояния из проекции, если `cells.length === fileCount`.
12. **Дерево выбранного мода** — `createTreeModeRenderPayload` V486 (109 569) → `buildConflictProjectionForGroups` (86 434, `conflict-tree-all-chains-v533`)
    → `renderConflictAwareGroupedLinesV486` (ASCII-дерево). Группы мода — `getConflictPresentationGroupsForModV680` (82 265): LG → кеш → backend
    `getConflictGroupsForMod` (82 035: LG → per-mod кеш v684 → срез страницы → ленивый prefetch) → проекция v669 при свежей линии.
    Оверлей win/lose файлов — `loadTreeConflictOverlayFromIdb` (82 787) / `buildTreeConflictOverlayFromGroup` (82 930); класс строки — поражение важнее победы (83 049).
13. **Бейджи** — `getModOverwriteContextBuckets` (81 347) → `groupConflictModalGroupsByContext` → `projectConflictBadgeBucketsToPresentationV669` (151 378)
    → `renderModOverwriteContextStrip` (81 416); «OW N» — `renderModOverwriteSummaryStrip` (81 755) по `overwriteModSummary`. Счётчики связей — v645 (116 549).
14. **Фокус** — `setConflictFocusState`: живая цепочка обёрток V486 (109 772) → V488 (87 896) → V487 (87 084) → делегат (109 190, owner 85 912).

**Какое переопределение побеждает** (сводно):

| Имя | Назначения (порядок исполнения) | Живое |
|---|---|---|
| `modlistToolMaterializeOverwriteNormalizedLayerV501` / `AnalysisPathStore.materialize…` | 125 625 v501 → 126 781 v502 → 149 559 v660 | **v660** |
| `modlistToolInvalidateOverwriteNormalizedLayerV501` | 125 626 → 126 768 (снимок v502) → 149 553 v660 | **v660** |
| `AnalysisPathStore.getOverwriteChain` и ещё 4 чтения | методы IIFE (S13) → 125 594–125 622 | **v501** (нормализованный при готовности) |
| `AnalysisPathStore.updateOverwriteChainIdentityField` | 121 659 → 126 802 | **v502** (пишет `overwriteNormalizedChains`) |
| `loadConflictModalGroupsFromIdb` / `loadConflictGroupsForModFromIdb` / `getConflictModalGroupByKeyAsync` | decl 144 236 / 144 344 / 152 385 → 125 956 / 126 009 / 126 068 | **поздние присваивания** |
| `decorateConflictGroupsWithIdentity` | decl 138 133 → 164 575 | **GraphV476**; пайплайн IDB зовёт захваченную legacy |
| `decorateOverwriteContextsWithIdentityV468` | 138 677 → 164 587 | **GraphV476** (в стадии contexts — свой проход v468) |
| `setConflictFocusState` | 109 190 → 87 085 V487 → 87 896 V488 → 109 772 V486 | **V486** (внешняя) |
| `getConflictGroupFilePaths` | 88 068, 143 199 | **143 199** |
| `renderVisualTreePanel`, `createTreeModeRenderPayload`, conflict-only панели | decl 135 585 / 135 842 / 88 270 / 88 295 → 109 778–109 802 | **V486** |

## 3. Модель данных

Все IDB-ключи — `projectId::runId::<id>` (keyPath `key`), чтение курсором по `getIdbPrefixRange('p::r::')`; `meta`/`overwriteMeta` — одна запись `p::r`.
Роль: **К** — каноническое (единственный источник), **П** — производное (пересобирается из К), **Р** — рабочее/временное, **UI** — проекция для показа.

### 3.1. Пути и метаданные (обл. 6)

| Store | Ключ | Поля (главное) | Роль |
|---|---|---|---|
| `modPaths` | `p::r::mod` | `storage:'chunks'|'cold-segment'`, `chunkPrefix/Count/Size`, `pathCount`, `samplePaths≤40`, манифест `path-storage-manifest-v1` (`pathStorageMode idb-chunks|dual-idb-mnlth|mnlth-cold-segment`, `pathStorageHotCopy`, `coldSegmentRef{kind,segmentId,byteOffset,byteLength,pathCount,contentHash}`, `pathContentHash`), `routeMetadata*` | К (манифест) |
| `modPathChunks` | `p::r::mod::chunk::NNNNNN` | `paths[≤1000]`, `pathCount` | К (горячая копия) или удаляется при cold |
| `modRouteMetadata` / `…Chunks` | ключ мода / `…::route-meta::NNNNNNNN` | манифест v2 (`digest sha256 'route-metadata-v555'`, coverage, `indexAligned`), записи `{index, originKind, surfaceRole, byteLength, modifiedAt, contentHash, sourceRef, sourceCount, shadowedSourceCount, metadataStatus}` | К; теряется при переводе в cold |
| `modAnalysisState` | `p::r::mod` | сигнатуры поверхности, `pathsStored`, возможности v517 | К |
| `meta`, `analysisGenerations`, entity-surface v552 (4 stores) | — | поколения/печати пробега (обл. 5); v552 API не подключён к продукту | — |
| `.mnlth` `cold-path-segment` | `segmentId` | `modName, projectId, runId, pathCount, contentHash, payload.paths` | К для холодных модов |

### 3.2. Граф перезаписи (обл. 6/10)

| Store | Ключ | Поля | Роль |
|---|---|---|---|
| `conflictPathDict` | `p::r::<pathId>` | `pathId, path (lower), root, shard (hex2), bucket 'root::shard'`, `storageMode 'conflict-path-dict-v1'` | К (только конфликтные пути после reduce owners) |
| `overwritePathOwners` | `p::r::pid::<pathId>` (legacy `p::r::<path>`) | `pathId, pathRoot/Shard/Bucket, ownerRefs[[modId, modName, sepId, sepName, priority, enabled01]] (priority↑), ownerCount` | К (снимок priority/enabled!) |
| `overwriteModSummary` | `p::r::mod` | `conflictPaths, wins, losses, samplePaths≤12, opponentNames≤32, opponentRefs, opponentsCount(=refs), priority, enabled` | П |
| `overwriteModFiles` | `p::r::mod` | `conflictPathIds (sorted), samplePaths≤12, pathCount, priority/enabled (первый вклад)` | П |
| `overwriteChains` | `p::r::<chainId>` | lite v2: `id 'owchain_…', signatureHash 'owstack_…', exactStackSignature, exactChain:true, route, routeSignature, activeRoute(+Signature), activeWinnerModName, baseModId/Name, filesCount, samplePaths≤12, participants (только счётчики), fileStacks≤4 lite, stackCount=1, mergedFromChainIds, meta, identity, pathChunks{chunkCount, pathIdsCount, compactPaths}` | К (точный факт) |
| `overwriteChainPathChunks` | `p::r::<chainId>::chunk::NNNNNN` | `pathIds` (v3) или + `paths` (v2, при `legacyFullOverwriteChainPathChunks`) | К |
| `overwriteChainByMod` | `p::r::mod` | `chainIds (sorted), chainCount` | П (индекс) |
| `overwriteRouteGroups` | `p::r::rg_…` | schema `overwrite-route-group-v2`; exact: `kind 'exact-route', route, routeSignature, mainChainId, chainIds≤64 (sample!), chainCount, filesCount, stackCount, similarityRouteGroupId`; similarity: `kind 'route-similarity', reason, mainRouteGroupId, childRouteGroupIds, routeVariants[]` | П |
| `overwriteRouteGroupByChain` | `p::r::<chainId>` | `exactRouteGroupId, similarityRouteGroupId, routeGroupIds[]` | П |
| `overwriteContexts` | `p::r::<ctxId>` | lite v1: `generation 'ctxgen_…', componentLabel, identityFingerprint, chainIds (все), chainFingerprints≤64, modNames, modIds, sampleFiles≤32, filesCount, stats{chains,mods,files}, identity, ui{customName/Icon/Color, collapsed}, lifecycle{matchedFrom, matchScore, mergedFrom}, order` | П, но несёт ручные поля (de facto К для имён) |
| `overwriteContextByChain` | `p::r::<chainId>` | `generation, contextId, contextFingerprint, contextName` | П |
| `overwriteContextView` / `overwriteChainPreview` / `overwriteModView` | id / chainId / mod | UI-лайт: title/icon/color, sample≤32/≤8, preview с `contextId`, `sourceSafeGroup*`, `mergeProposal`, ≤2 стека; modView — корзины контекстов `{contextId, chainIds, groupCount, filesCount, stackCount}` | UI |
| `overwriteMeta` | `p::r` | статус стадии, вся статистика, `telemetry` (v503) | диагностика |
| `overwriteWorkPartitions` | `p::r::work::<stage>::partition::<n>::chunk::<12>` | `items` вкладов | Р (не экспортируется) |

### 3.3. Нормализованный слой v501 (обл. 10) — 6 stores `overwriteNormalized*`

| Store | Ключ | Поля |
|---|---|---|
| `…Chains` | `p::r::ownorm_…` | `truthFingerprint 'normtruth_…', sourceExactChainIds, sourceTruthFingerprints, files (объединение), fileStacks (все стеки источников, id `<id>_stack_k`, `oldChainId`), participants (buildParticipantsV501), route, activeRoute, winner, contextId, sourceIdentityCandidates, sourceSafeGroupIds/Groups (v652/v653/v657), mergeProposal (v653, readOnly), identity (fingerprint = truthFingerprint), exactFileStackTruth:true, migrationV502` |
| `…ChainByMod`, `…ByExact` (exactId→normId), `…ContextByChain` | — | индексы |
| `…Contexts` | `p::r::<ctxId>` | `sourceContextIds, chainIds, modNames, stats (files с пересечениями), identity` |
| `…Meta` | `p::r::manifest` | `ready, active, exactChains, normalizedChains, normalizedContexts, filesCount, fileStacks, repeatedAppearances, sourceProof, modlistOrderDigestV658, normalizedPagePreviewSchemaV687, normalizedPagePreviewsV687, lightPageReadReadyV687, durationMs`; + `p::r::page-preview::<id>` (превью v687); + `p::r` = `migrationV502` (отдельная запись!) |

Зеркало в проекте: `runs.treeFull.overwriteNormalizedChainsStore {ready, active, projectId, runId, rawStats}`, `overwriteNormalizedChains` (лёгкий, `chains:[]`).

### 3.4. Сущности в памяти

- **Owner ref**: `{modId, modName, separatorId, separatorName, priority, enabled}` + оверлей `storedPriority/storedEnabled, livePriorityApplied/Changed, liveEnabledChanged, livePriorityMissing`.
- **Стек** (`makeOverwriteStackDraft` 116 347): `{id 'owstack_…', signature, files, route (все владельцы, priority↑), activeRoute, activeWinnerModName, ownerRefs}`.
- **Участник цепочки**: `{modName, priority, enabled, chainFilesOwned, missingChainFiles, extraConflictFiles, wins, loses}` (+`*Count`); в lite — только счётчики.
- **Conflict group** (`convertOverwriteChainToConflictGroup` 142 591) — единая UI-форма: `{source, normalized, key=chain.id, pathKey, contextId, title, shortTitle, mods, chain[участники], winner, losers, paths, samplePaths≤8, count, activeCount, stackCount, fileStacks, sourceExactChainIds, sourceSafeGroup*, mergeProposal, overwriteChain, identity, icon, color, fingerprint, conflictProjection}`.
- **Route group** (exact / similarity) — см. 3.2; в cutover-финализаторе CP10 (172 897, тень) — предок `owroute_*`.
- **Safe group v499** (`sourceSafeGroups[i]`): `{id 'safe_exact_…'|'safe_dag_…'|'repeat_cover_…', route (топологический супер-маршрут), winner, filesCount, stackCount, sourceChainIds, sourceTruthFingerprints, safeProof, mergeHistory}`.
- **Merge proposal v652**: `{id 'owmerge_<fnv(sorted safeIds)>', rebuildable:true, userAccepted:false, sourceSafeGroupIds, sourceChainIds, route, winner, filesCount, stackCount, repeatCoverProof}` — только при ≥2 safe-группах.
- **LG-цепочка v565** (`buildLogicalGroupNGEffectiveChainV565` 51 789): id `ngchain_<digest32>`, участники — эффективные единицы (композит `mod:composite-mod:<id>`), один стек, `wins/loses` = весь набор путей; контексты `ngctx_<digest32>` BFS по общему участнику.

### 3.5. Состояние UI и идентичности (персистится в `state.project.ui`)

| Поле | Ключ записи | Содержимое | Роль |
|---|---|---|---|
| `ui.conflictDone {chains{key→bool}, contexts{sig→{cardMode,…}}}` | id цепочки / подпись контекста | legacy Done | К до v665, далее мигрируется |
| `ui.overwriteResolutionRecordsV665 {schema, revision, items}` | `owrestruth_<hash>_<n>` | `{id 'owres_…', sourceTruthFingerprints, done, doneAt, origin{kind presentation|legacy-v501-done, legacyChainKeys}}` | **К (Done)** |
| `ui.overwritePresentationIdentitiesV666 {…items}` | `owcomptruth_<hash>_<n>` | только **запертые** поля name/icon/color + locks | **К (ручные имена)** |
| `ui.overwriteUserCompositesV657 {schema, revision, items}` | `id 'owcomp…'` | `sourceTruthFingerprints, membershipFingerprint, sourceGroupTruthSignatures, identity, origin{proposalId}` | **К (принятые слияния)** |
| `ui.conflictSolutionFront`, `front.presentationV667` | — | фокус фронта решений, якоря членства | К (UX) |
| `ui.conflictDoneMigrationV502`, `ui.overwriteResolutionMigrationV665` | — | журналы миграции (partial/unmapped) | журнал |
| `analysisConfig.conflictIgnorePaths` | — | отсортированный набор нормализованных правил | **К (игнор-лист)** |
| `state.conflictsUi.conflictFocus` | — | `{chainKey, modName, filePath, fileIndex, presentationEntityKey, presentationMembershipFingerprint, presentationContextId, revision}` | runtime |
| `state.conflictsUi.presentationSelectionV662` | — | выбор presentation-сущности + `backendChainId` | runtime |
| Реестр идентичности v461 (`WeakMap project → scope → owners`) | ownerKey | занятые имена/иконки/оттенки/OKLCh | runtime, **не сохраняется**, пересобирается при decorate |
| Граф идентичности v476 | ownerKey | кеш контекстов графа, очередь миграции | runtime |

**Presentation-сущность v661** (`buildConflictPresentationEntityV661` 149 631): `{key, type user-composite|safe-chain|backend-chain, contextId, mods, winner,
sourceTruthFingerprints, sourceExactChainIds, sourceSafeGroupIds, backendChainIds, identity (+v666), resolution{owner, status, done}, proposal, details{state inline|summary-only, requiresBackendFetch}}`.
**Атом истины** = `truthFingerprint` точной цепочки (`owtruth_…`, §4.10); сущность = множество атомов; его отпечаток = ключ Done/имён/композитов.

## 4. Точные правила и формулы

`fnv` = `hashStringToInt` (136 021): FNV-1a 32 бит, offset 2166136261, prime 16777619, `>>>0`; `b36` = `.toString(36)`.

### 4.1. Нормализация путей и id
- **Путь анализа** `normalizeAnalysisPath` (113 557): `\`→`/`, схлопнуть `//`, срезать ведущие/хвостовые `/`; **регистр сохраняется**. Списки — uniq + sort `localeCompare(sensitivity:'base')`.
- **Путь конфликта** `normalizeTreeConflictPath` (143 074): `\`→`/`, срезать префикс `selected/` (без учёта регистра), ведущие `/`, **lower-case**. Это каноническая форма везде в обл. 8.
  Пути цепочки дополнительно сортируются `.sort()` по кодовым точкам (`normalizeOverwriteChainPathList`) — порядок отличается от `localeCompare`.
  v501 `normalizePathV501` — без lower-case (только trim и слэши).
- **Хэш списка путей** `hashAnalysisPathList` (113 937): токен `${len}:${path}\n`; по символам `h1 = imul(h1 ^ c, 16777619)>>>0` (старт 2166136261),
  `h2 = (imul(h2 ^ c, 2246822507) + i + j)>>>0` (старт 16777619; i — индекс пути, j — индекс символа); `pathlist_v1_<n>_<h1 b36>_<h2 b36>`.
- **pathId** (114 992): `cp_<fnv(norm) b36>_<fnv(len+'::'+reverse(norm)) b36>_<len>`; shard = `fnv(pathId) % 64` в hex(2); root = первый сегмент lower или `(root)`.
- **Игнор-лист** `isConflictPathIgnored` (143 212), правило нормализовано: пустой путь → игнор; точное совпадение; правило без `/` — имя файла (`…/rule` или корень);
  правило с хвостовым `/` — папка в начале или середине (`rule…` / `…/rule…`). Glob нет. Применяется в стадии owners (и RAM `buildConflictCache`).
  Смена листа → `refreshOverwriteConflictModelAfterIgnoreChange` (143 155) → IDB: `rebuildOverwriteConflictModel` (154 276), RAM: пересборка.

### 4.2. Приоритет и победитель
- **Приоритет хранения** (`buildProjectModLookup` 142 019): обход `separators` (пропуск **только SYNC**), `getRealSeparatorMods`, сквозной счётчик с 1, `enabled: !!mod.enabled`.
- **Живой оверлей** (`getLiveOverwriteOwnerLookup` 116 207, кеш по версии проекта): пропуск **всех функциональных** разделителей (`isFunctionalSeparator` 38 986:
  SYNC + PLUGINS + донор), `enabled = mod.enabled !== false`. `applyLiveOverwriteOwnerPriority` (116 262): по modId, затем по имени; найдено — priority/order/enabled/separator
  заменяются живыми, старые в `stored*`; не найдено — `livePriorityMissing:true`, остаются сохранённые.
- **Порядок владельцев** `orderOverwriteOwnersByPriority` (116 295): оверлей → без имени отброшены → priority ↑, тай `modName.localeCompare(base)`.
- **Победитель** `getActiveOverwriteWinner` (116 313): **последний включённый** в этом порядке; если включённых нет — нет победителя (стек без winner).
  RAM `getRamActiveOverwriteWinner` (142 166) — то же. Legacy UI (`getConflictPathWinnerNameForGroup` 88 254, `refreshConflictPresentationDetailOrderV703` 85 512,
  `decorateConflictGroupByCurrentOrder` 141 524): последний включённый, **иначе последний вообще**. v663/v654: priority = живой `getGlobalPriorityByModName()` или index+1.
- **Live vs stored**: цепочки при чтении (`getOverwriteChainActiveWinnerName` 117 222) — живой > `chain.activeWinnerModName` > последний активного маршрута.
  Сводки `overwriteModSummary` из owners считают wins/losses по победителю на момент пробега (оверлей применён к `buildProjectModLookup`-снимку того же момента);
  `rebuildOverwriteModSummaryFromOwners` (119 081, только импорт/восстановление) — по живому оверлею. `overwriteModFiles.priority/enabled` — снимок первого вклада.
  Идентичность точной цепочки (сигнатура) — **по сохранённому** порядку: после перестановки цепочка с id X описывает стек, который живьём уже другой.

### 4.3. Стек, точная цепочка, участник
- `signature = 'route:' + all.join('|||') + '###active:' + active.join('|||') + '###winner:' + winner` (116 331; all/active — имена в порядке priority↑).
- `stackId = 'owstack_' + b36(fnv(signature))`; `chainId = 'owchain_' + b36(fnv(signature + '###' + sortedNormPaths.join('|||'))) + '_' + paths.length` (117 147).
- Map (120 399): owner-запись с `pathId` и ≥2 владельцами → оверлей+сорт → вклад `[signature, pathId, compactOwners, winner]` в партицию `fnv(signature)%256`.
  Reduce: дедуп pathId, пути из словаря; **одна цепочка = один стек** (`fileStacks:[stack]`, `stackCount:1`).
- Участник: `chainFilesOwned = |paths|`, `extraConflictFilesCount = max(0, modPathCount − |paths|)` (modPathCount из `overwriteModFiles`), `wins = |paths|` у победителя, иначе `loses = |paths|`.
- Гидратация (`hydrateOverwriteChainFullDetails` 115 507): пути из чанков, `buildOverwriteStacksForPaths` (по одному IDB-get на путь), `missingChainFiles`/`extraConflictFiles` по `overwriteModFiles.conflictPaths`.
- Сводка мода: `conflictPaths, wins (победитель), losses (иначе)`, sample ≤12 лексикографически, соперники ≤32 (первые по `(path, order, name)`), `opponentsCount = Σ ссылок`.

### 4.4. Route groups (стадия `chains-normalize`, 117 237–118 050)
- Exact: по **живой** `routeSignature` (`getOverwriteChainParticipantRouteSignature`, все участники `|||`) → суммы chain/files/stack, `mainChainId` = max filesCount (тай — меньший id);
  `id = makeOverwriteRouteGroupId('exact', sig)` = `rg_<kind>_<fnv(kind::seed) b36>_<fnv(len::reverse(seed)) b36>_<len>`. `chainIds` — выборка ≤64 (членство — в `…ByChain`).
- Similarity: только внутри одного победителя (партиция `fnv(winner)`); элементы с route ≥2 и победителем; сорт: длина маршрута ↓, filesCount ↓, id. Жадно 3 прохода
  (элемент берётся один раз): (1) `subroute-absorption` — упорядоченная подпоследовательность главного, строго короче; (2) `common-tail` — общий суффикс ≥2;
  (3) `shared-route-edge` — ≥1 общее ребро соседей. Группа при ≥2 членах; id `rg_similarity_…(reason::sorted childIds)`.
- Точный слой не переписывается (`exactChainsRewritten:0`); счётчики старого «сливающего» нормализатора остаются нулями.

### 4.5. Контексты (стадия `contexts-build`, 119 231)
- Граф: вершины — цепочки, ребро — общий мод (`overwriteChainByMod`). Метка = chainId; итерации по строкам мод→chainIds: всем цепочкам строки ставится **min-label**
  (`localeCompare`); до `changed==0`, предел `min(4096, chains+1)` (иначе throw «did not converge»).
- Компонента → черновик `{chainIds (sorted), chainFingerprints≤64, modNames, modIds, sampleFiles≤32 (лексикогр.), stats{chains,mods,files=Σ}}`;
  `identityFingerprint = b36(fnv(chainFingerprints||chainIds ## chainIds ## modNames ## sampleFiles≤80 ## stats…)).upper` (118 125).
- **Наследование** (118 220 / 119 544): кандидаты — прежние контексты цепочек **того же `p::r`**; `score = 0.65·max(J(chainIds), J(chainFingerprints)) + 0.30·J(modNames) + 0.05·J(sampleFiles)`,
  J — Жаккар (0 при двух пустых). Сорт score↓/id; берётся первый **≥ 0.28**, не занятый (каждый старый — максимум одному новому); `mergedFrom` — прочие ≥0.28 (≤8).
  id = старый || `owctx_<fp lower>_<chainCount>` (уникализация `_2`, `_3`). Ручные `customName/Icon/Color` переносятся. Порядок контекстов — хеш-порядок партиций.
- Имя-фоллбек: 1 цепочка → её имя; 2 → `A + B`; >2 → `A + B + (n−2)`; без имён → `N chains · M mods`; обрезка 92 + `…` при >96.
- Поколение `ctxgen_<now>_<rand6>`; `sweepStaleGeneration` удаляет старые записи **после** записи новых.
- В RAM/таблице (144 600) контекст = BFS по двудольному графу цепочка ↔ **проецированная единица** (LG-композит схлопывает членов); Done-цепочки исключаются без `includeDone`.

### 4.6. Идентичность (v459–v468, v476)
- Контракт v460 (136 084): `{schemaVersion, name, color, icon, fingerprint, sourceFingerprint, generated (деф. true), locks{name,color,icon} (деф. !generated), scope, sourceType, mutation*, generatorVersion}`;
  scope `overwrite` для `overwrite-*`/`conflict-*`.
- Политика обновления `classifyProjectIdentitySourceStateV465`: `generate` | `preserve-manual` (generated=false) | `backfill-source` | `preserve-generated` (отпечатки равны) | `regenerate-source-change`
  (заблокированные поля сохраняются `mergeProjectIdentityRegeneratedFieldsV465`).
- Двухфазность v462: intrinsic-запись при пустом взгляде на коллизии; генерируемые сортируются (fingerprint → sourceFingerprint → ownerKey) и разрешаются по порядку — детерминизм не зависит от входа.
- Имя цепочки (137 891): старт `A → B → …`; `basePasses = count≤2 ? 0 : min(10,(count−2)·2)` × depth; веса признаков `collectConflictGroupScoreMap`: мод `4+(3 крайний|1)`,
  разделитель слова 3 / целиком 5, topFolder 8, secondFolder 5, расширение 7, слова файла 3, папки `max(1,4−idx)`, `multi file overwrite` при ≥10 файлах, `long chain` при ≥4 модах.
  Отпечаток `b36(fnv([key, mods, первые 80 путей, top-20 сигнатур, count].join('##'))).upper`. Уникализация: `base · FP4`, `base · FP4-2`…
- Цвет: hue = `fnv(seed)%360`; s/l по плотности; коллизии — OKLab (min 0,07, адаптивно `max(0.018, min(req, req·sqrt(40/max(40,N+1))))`, 272 попытки, шаг золотым углом 137,508°);
  фоллбек цепочки в IDB (118 149): `hsl(fnv(fp::chain)%360, 70%, 42%)`, ≤16 сдвигов при <18°; иконка из 12 эмодзи по `fnv(fp::icon)`.
- Контекст v468: score-map из имён цепочек (вес `8+min(10,round(ln(1+files)·1.5))`), модов 5, путей 3; `basePasses = clamp(4..10, 4+round(log2(chains+1)·2))`;
  `sourceFingerprint` включает имена/цвета цепочек → переименование цепочки ⇒ `regenerate-source-change` контекста. Перед контекстами резервируются идентичности цепочек (`overwrite-chain:<id>`).
- v476 (164 559): если у **всех** элементов есть кеш контекста графа — декорирование из графа (сигналы v472 через `ProjectRelationGraph.resolveIdentityContextDetailed`),
  иначе legacy без тени + очередь миграции (пакеты ≤128, затем `renderAll`). Граф v469–v474: ярусы mod/separator/plugin/path/tag = 0, LG/chain = 1, context = 2;
  наследование идентичности только снизу вверх; генерированная идентичность корня не кормит саму себя (барьер v473); бюджет B∈[0,10] (деф. 5): `budget=8·B/5`,
  `minSignalWeight=0.05·4^((5−B)/5)`, `maxVisited=256·B/5`, hopDecay 0,85.

### 4.7. Нормализованный слой: v498 → v499 → v500 → v501 → v502
- **v498** (только диагностика): почему старый сливающий нормализатор опасен — циклы маршрутов (Кан) и смешение победителей.
- **v499 safe route-DAG** (122 347–123 260). Слияние допустимо, если: общие моды >0, связность (BFS по общим модам), один ключ победителя
  (`winner:X` | `mixed:…` | `winner:unknown`), объединение рёбер ациклично, каждый исходный маршрут сохраняется как подпоследовательность супер-маршрута,
  ссылки на источники не пересекаются, узлов ≤256, вариантов ≤128. Отказы по приоритету: `empty-candidate → disconnected-routes → winner-mismatch → route-cycle →
  source-route-not-preserved → duplicate-source-reference → route-node-cap|route-variant-cap`. Супер-маршрут — Кан с тай-брейком `minRouteIndex → firstVariant → firstPosition → имя`.
  Старт: точные группы по ключу `winnerKey::routeSignature` (`safe_exact_…`). Кандидаты: корзины `winnerKey::mod`, ≤6 якорей + окно ±3, кап 180 000 рёбер, subroute-пары обязательны.
  **Ранжирование**: `tier = subroute 4 | sharedEdges>0 3 | commonTail≥2 2 | 1`; `score = tier·1e6 + sharedMods·1e4 + round(J·1000) + min(999, exactA+exactB) − 10·max(0, unionMods − maxRouteLen)`;
  по score↓ union-find + incremental assess (≤250 000 оценок).
- **v500 repeat-cover** (123 261–123 903): цель лексикографически `repeatedAppearances (Σ(count−1) по модам) → chainCount → routeComplexity (Σ(len+2·(variants−1))) → routeVariants → totalModAppearances`, меньше — лучше.
  Компоненты union-find по `winnerKey::mod`. Компонента ≤9 групп — beam (ширина 48, ≤2400 состояний, ≤12 000 оценок, принимается только строго лучшее);
  больше — greedy (≤24 прохода, ≤120 000 оценок; бакет ≤18 — все пары, иначе 8 якорей + окно 3; `score = sharedMods·1e6 + min(9999,|srcA|+|srcB|) − 100·addedComplexity`).
  Гарантии: `noWinnerMixing`, `repetitionNeverWorseThanSafeBaseline`, `sourceSafeGroupIdsPreservedExactlyOnce`, `mergeProposalMembershipDisjoint`. Происхождение: `sourceSafeGroupIds` (v652) транзитивно.
- **v501 materialize** (125 286): обязательны `sourceExactChainIdsPreserved / aggregateFilesPreserved / aggregateStacksPreserved`; число кандидатов = chainCount стадии
  `repeat-minimized-cover`; на каждую точную цепочку синтезируется стек `<id>_exact_stack`; доказательство: Σ|sourceExactChainIds| = |exact ids| и Σ стеков = Σ стеков точных.
  Инварианты слоя: каждая точная цепочка ровно в одной нормализованной; стеки не теряются; победитель внутри нормализованной цепочки один; точные stores не меняются.
  Готовность лёгкого чтения (124 931): `freshness.current ∧ manifest.ready ∧ lightPageReadReadyV687 ∧ schema == v687 ∧ previews ≥ normalizedChains`.
- **v502 идентичность** (125 051, 126 135): `truth(stack) = sort(owners).join('|||') + '###' + sort(files).join('|||')`; точная цепочка —
  `owtruth_<b36 fnv(sorted stack seeds.join('@@@'))>_<stackCount>`; нормализованная — `normtruth_<fnv(sorted source truths)>_<n>`, id `ownorm_<fnv(sorted truths|sourceIds)>_<n>`.
  Победитель, приоритет и маршрут в идентичность **не входят**. Сопоставление поколений: цепочка — truth == → множество source-id == → `2·overlap(truths) + overlap(sourceIds) ≥ 1.2`
  (`overlap = |A∩B| / max(|A|,|B|)`); контекст — id == → сигнатура sourceContextIds == → `overlap ≥ 0.6`. Выбор кандидата имени: `manualScore` (+8 за custom/manual поле,
  +5 за lock, +4 если generated=false и score>0) ↓ → есть sourceFingerprint → id; поля `custom > manual > identity > meta`.
  Done: нормализованная цепочка Done, **только если Done все её точные источники**; частично — в журнал `partialChains`.
- **v660**: ключ поколения `'v501-generation-v660|pid|rid|builtAt|modlistOrderDigestV658|exactChains|normalizedChains'`; смена → сброс фокуса/дескрипторов предложений.
- **Свежесть v658**: `oworder_<b36 fnv(fp)>_<count('|')>_<fp.length>` от `getCurrentModlistOrderFingerprint()` (`join('|')` по `${sepId}:${modId}` не-SYNC);
  материализованный дайджест ≠ текущему ⇒ линия v499 несвежая ⇒ карточки «Расчёт устарел», лёгкие превью отключены, предложения слияния заблокированы.

### 4.8. Проекция v484/v490/v533 и «зубной ряд»
- Ось файлов (без пересортировки, uniq по норм. пути): `chain.files → group.paths → group.files → fileStacks.files → samplePaths → owned/wins/loses участников`.
- Участники: `group.chain → overwriteChain.participants → group.participants → group.mods`; `order` = глобальный живой приоритет, иначе `priority/order/index+1`; сорт order, имя.
- Индекс стеков: `byPath[path]` — **первый** стек, содержащий путь; `winnerModName` — только если среди owners; `resolved = owners≥2 ∧ winner`.
- Файл: `{path, fileIndex, ordinal, shadeSlot=i mod 24, owners, activeOwners, winnerModName, loserModNames, resolution ∈ stack-resolved|participant-resolved|stack-unresolved|participant-unresolved|unknown}`;
  победитель: из стека → единственный явный `wins` → ''.
- **Клетка** участника × файл: не владелец → `missing`; есть победитель файла → `win|lose`; иначе по явным `wins`/`loses`; оба или ни одного → `unknown`.
- Сводки: `expectedFileCount = chain.filesCount|group.count|files.length`, `complete = !summaryOnly ∧ fileCount ≥ expected`.
- Хэш v490: FNV-1a 32 с байтом 0xff после каждой части, hex(8); `runtimeStamp = 'v490:' + hash(…ревизия, identity, файлы, стеки, участники…)` — ключ кеша.
- Мульти-цепочка v533 (86 434): путь включается, если среди owners выбранный мод (или его LG-члены), или клетка `isOwner`, **или нет доказательств владения вовсе**;
  объединение по пути, сорт `localeCompare(base, numeric)`, primary = первое членство.
- **Зубы**: 1 файл = 1 сегмент в порядке оси; объявленные состояния `w` (в wins) / `l` (loses) / `u` (owned, но не w/l) / `m` (missing); при наличии проекции —
  `win→w, lose→l, unknown→u, иначе m`; линейка — `r`. Цвета: w `rgb(67,142,96)`, l `rgb(181,82,82)`, u `rgb(66,128,153)`, r `rgb(112,126,145)`, m `rgba(72,84,101,.94)`.
  Подпись `files=N · exact 1:1 · W L M X` — **W/L/M/X = wins / loses / missing / extra** (цвета #86efac/#fca5a5/#94a3b8/#fcd34d).
- Палитра файла v490: slot = (i·5) mod 8, lightOffset `[-4,-1,2,5,-3,0,3,6][slot]` от цвета цепочки; 12 оттенков в дереве (`fileIndex % 12`).
- Таблица контекста: ячейки `win|win-linked|lose|lose-linked|owned|extra|missing`; колонка-стек: `activityScore = strong·100 + linkedStrong·70 + owned·30 + activeMods·10`;
  группировка запечённых стеков по `signature|routeSignature|paths`.
- Оверлей дерева (82 930): мод «выигрывает» путь, если он победитель **или** путь в его `wins`; «проигрывает», если победитель другой **или** путь в `loses` (оба возможны);
  состояние распространяется на все префиксы-каталоги; класс строки — lose важнее win.

### 4.9. LG-стор v565/v571 (где перекрывает чтения)
- Если в проекте есть активные логические группы, `getLogicalGroupNGAuthoritativeConflictStoreV571` (53 958) возвращает стор v565 (или пустой pending), и он
  **первым** читается в `getStoredOverwriteChains`, `getConflictModalGroups`, `getConflictGroupsForMod(Async)`, `loadTreeConflictOverlayFromIdb`, `getOverwriteContextForChainNowRaw`.
  При pending: `[]` для LG-сущностей и `null` для обычных модов (v643: вызывающий падает на IDB/RAM).
- Стор строится атомарно (любой blocked-участник → весь стор не строится), гейты: граф `ready-shadow` с `parity.ok`, канонический v577, совпадение `analysisProjectId`.
  Участник — эффективная единица, `priority = order` (сквозной индекс по эффективному списку, **не** `getGlobalPriorityMap`); в проекции v571 `priority` композита = **min** приоритетов членов.
- id цепочки `ngchain_<digest32(routeKeys, activeKeys, winnerKey)>`; имя санитизируется, если содержит имя любого участника (подстрокой).

### 4.10. Presentation (v653–v702): отпечатки и отказы
- `makeConflictTruthMembershipFingerprintV657(fps) = 'owcomptruth_' + b36(fnv(sorted.join('|||') || 'empty')) + '_' + n` — ключ имён v666 и композитов.
- `makeOverwriteResolutionMembershipFingerprintV665 = 'owrestruth_…'` (та же формула) — ключ Done и фронта v667. Done — точное совпадение множества атомов (подмножество ≠ Done).
- Фильтр шапки v670: `owtruth:<b36 fnv('v670|'+sorted)>.<b36 fnv('v670r|'+reversed)>.<n>`.
- Композит v657: ≥2 членов, у всех есть атомы (иначе `truth-lineage-unavailable-rematerialize-v501`); идемпотентность по membership; пересечение атомов с другим
  композитом запрещено. Сверка: `ORPHANED` (нет групп) / `AMBIGUOUS` (группа с атомами вне композита или >1 контекста) / `PARTIAL` (не все атомы) / `EXACT` / `REDISTRIBUTED`; resolved = EXACT|REDISTRIBUTED.
- Коммит предложения v695 — отказы по порядку: `proposal-focus-descriptor-stale → …-incomplete → v499-lineage-stale → proposal-commit-guard-missing → proposal-generation-changed →
  …-revision-changed → proposal-modlist-order-changed → proposal-materialized-order-changed → proposal-current-order-changed → proposal-membership-changed → proposal-truth-lineage-unavailable`.
- Фронт решений v667: связность `Σ(+10 общий мод, +2 общий файл) + |mods| + min(30, files/4)`; следующий: +80/+40 за мод в решённых/последней, +8/+4 за файл.
- Карточка safe-группы v654: маршрут по текущему глобальному приоритету, `winner = последний enabled`, цвет `hsl(|fnv(safeId)|%360, 54%, 53%)`, proposal копируется только при свежей линии.
- Советник порядка (145 070): score 50; base/framework/core… −22; patch/compat/fix… +26; output/generated/bodyslide/fnis/nemesis/dyndolod/xedit… +42; `lose>2·win ∧ lose≥4` −6;
  `win>2·lose ∧ win≥4` +6; `useCurrentTruth`: `0.45·score + 0.55·clamp(priority,0,100)`; переставлять при `|score − priority| ≥ 26` (aggressive 14).
  Применение — внутри каждого разделителя (слоты сохраняются) или перенос в новый разделитель перед SYNC; SYNC-запись порядка.

## 5. Сравнение с NG (`crates/sat-core/src/overwrite.rs`)

NG: `OverwriteProjection::compute(table, priorities)` — чистая функция, параллельно по путям (части по 2^14), затем детерминированное интернирование стеков.

**Совпадает по смыслу**
- Стек на путь, конфликт = ≥2 различных мода в стеке; точная цепочка = множество путей с идентичным стеком; контекст = компонента связности мод↔цепочка
  (NG — union-find по модам, монолит — label propagation по цепочкам; разбиение одно и то же при одинаковом определении стека).
- Регистронезависимые пути (интернер NG сворачивает регистр; монолит — lower-case).
- «Свой архив не конфликт»: мод встречается в стеке один раз (NG `own_archive_copy_is_not_a_conflict`; монолит дедуп владельца по `modId::modName`).
- Перестановка не требует I/O: NG пересчитывает проекцию; монолит — живой оверлей + ленивый пересчёт порядка v495.

**Семантические различия (решить явно)**
1. **Loose vs archive.** NG: loose всегда выше архива, затем priority. Монолит: победитель только по priority (архивные пути — те же «пути мода» из `appendModPaths`);
   правило loose>archive есть лишь в слиянии метаданных **одного** мода (`mergeRouteMetadataEntriesV554`). NG ближе к игре; паритет с монолитом здесь не нужен.
2. **Выключенные моды.** NG: inactive (`priority < 0`) не входит в стек → путь «включённый + выключенный» не конфликт. Монолит: выключенные — владельцы,
   путь с ≥2 владельцами конфликтен даже при одном включённом; сигнатура содержит `route` (все) и `active` отдельно; победитель может отсутствовать.
   Следствие: у монолита больше цепочек/контекстов, а смена enabled меняет идентичность стека, но не набор конфликтов.
3. **Порядок стека.** NG `chain_mods` — победитель первым; монолит `route` — priority ↑, победитель последним.
4. **W/L/M/X.** NG: W — пути, где мод наверху; L — не наверху; M — посередине; X — `fully_overridden` (активен, нет побед и уникальных). Монолит в карточках:
   W/L/M/X = wins / loses / **missing** (файлы цепочки, которыми мод не владеет) / **extra** (его конфликтные файлы вне цепочки) — относительно цепочки, не мода.
   NG `+n` (unique) в монолите нет; «зубы» w/l/u/m — относительно оси цепочки. Буквы совпадают, смысл M и X — нет.
5. **Источник приоритета.** NG получает `Priorities` от вызывающего (одна шкала). Монолит — три шкалы: снимок `buildProjectModLookup` (пропуск только SYNC, `!!enabled`),
   живой оверлей (пропуск SYNC+PLUGINS+донор, `!== false`), `getGlobalPriorityByModName` в UI; LG — эффективный порядок и min членов.
6. **Id цепочки.** NG — индекс первого появления (нестабилен между проекциями). Монолит — хеш сигнатуры+путей (`owchain_…`), идентичность-истина `owtruth_…` (без порядка).
7. **Игнор-лист** в NG отсутствует (в `sat-core` нет `conflictIgnorePaths`).

**Чего в NG нет**: route groups; нормализованный слой (v499 safe-DAG, v500 cover, предложения слияния, композиты); идентичность (имена/цвета/иконки, реестр
коллизий, наследование контекстов по Жаккару, граф связей); Done/ручные имена по атомам истины; дельта «stored vs live»; Overwrite Order Advisor;
LG-композиты как единица конфликта; холодное хранение путей. Есть то, чего нет у монолита: VRAM/байты победителя, `fully_overridden`, `unique`.

## 6. Расхождения и вероятные ошибки

| # | Что | Где | Уверенность | Сегмент |
|---|---|---|---|---|
| 1 | Врезка декорирования `chainUpdates` внутри `makeContextResolver` (ветка `if (!nextId)`): `chainUpdates` нигде не объявлен (xref: 0 объявлений) → `ReferenceError` при первом несовпавшем старом контексте; v502-миграция падает **после** записи слоя v501: Done/фокус/выделения и `migrationV502` не переносятся | 126 468–126 498 | **подтверждено** (текст + xref) | S14 |
| 2 | `orderOverwriteChainParticipantsForConflictGroup`: `orderOverwriteOwnersByPriority` объявлена только в IIFE (116 295, depth 2) → с верхнего уровня `typeof` = undefined; фоллбек читает несуществующее `state.project.mods` → все участники `livePriorityMissing`, сортировка по **сохранённому** priority. Карточки после перестановки модов показывают старый порядок участников (кроме v703/v654, которые переупорядочивают сами) | 142 514 | **подтверждено** (текст + xref) | S16 |
| 3 | `auditOverwritePriorityFreshness` зовёт помощников из замыкания `AnalysisPathStore` → `ReferenceError` при первой записи | 131 285 | вероятно | S14 |
| 4 | Две шкалы приоритета: снимок (пропуск только SYNC, `!!enabled`) vs оверлей (SYNC+PLUGINS+донор, `!== false`) → ложные `livePriorityChanged/liveEnabledChanged`, сдвиг шкалы при модах в PLUGINS/донор-разделителе | 142 019 / 116 207 | подтверждено (по коду, последствия — вероятно) | S13 |
| 5 | Идентичность точной цепочки зависит от сохранённого порядка; после перестановки точный id описывает другой живой стек, а route groups строятся по **живой** `routeSignature` поверх сохранённых цепочек — смешанная шкала | 120 399, 117 802 | вероятно | S13 |
| 6 | `rebuildOverwriteConflictModel` (после смены игнор-листа) пересобирает точные цепочки, но не инвалидирует нормализованный слой v501 явно; ручной перезапуск 154 290 дважды строит цепочки | 154 276, 104 274 | вероятно (инвалидация в `runOverwriteOwnersStage` есть — проверить, проходит ли ветка) | S11, S17 |
| 7 | Ошибки оркестраторов не ставят статус `failed`: `overwriteMeta.status` остаётся `running`; контексты — смесь поколений без sweep | 120 780, 119 811 | подтверждено | S13 |
| 8 | `materializeV501` очищает 6 stores отдельными транзакциями до записи — не атомарно (безопасный откат на точный слой) | 125 286 | подтверждено | S14 |
| 9 | `assignAuthoritativeExactChainIdentities` грузит все цепочки в RAM — единственный неограниченный шаг | 120 704 | подтверждено | S13 |
| 10 | `readOverwriteChainPathBundleFromChunks`: при хоть одном прямом пути `pathIds` игнорируются | 115 486 | подтверждено | S13 |
| 11 | `get…Page(limit)` берут первые N по ключу, сортируют после — «первые N по order» не гарантированы | 118 931, 120 896 | подтверждено | S13 |
| 12 | Фоллбек-идентичность в read-views вызывается с пустыми usedNames/Icons/Hues — коллизии между безымянными цепочками | 118 680 | подтверждено | S13 |
| 13 | `buildConflictGroupsFromOverwriteModView` ждёт `context.groups`, а mod view их не пишет (живой — `…Lite`) | 118 994 | вероятно (мёртвая ветка) | S13 |
| 14 | Две метрики сходства для одной задачи: Жаккар 0,65/0,30/0,05 ≥0,28 (контексты в стадии) и `overlap/max` ≥0,6 / ≥1,2 (v502) | 118 220 / 126 371 | подтверждено | S13, S14 |
| 15 | Наследование контекстов в стадии ищет только в том же `p::r` — между пробегами имена держатся реестром/v502, не стадией | 119 479 | вероятно | S13 |
| 16 | `scanStore` v502: префикс `p::r` без `::` захватывает `run1` ⊂ `run10` | 126 262 | подтверждено | S14 |
| 17 | `migrateUi`: нерезолвленные выделения перезаписываются `''`, нерезолвленные контексты фронта теряются | 126 598 | подтверждено | S14 |
| 18 | Префиксы `owtruth_` (v501 точные) и `normtruth_` (v502 `truthFingerprint()` — без нормализации путей, только trim) — пересчёт может не совпасть с сохранённым | 125 054 / 126 230 | вероятно | S14 |
| 19 | Зубы: `u` = «владеет, исход не w/l» в объявленных данных и `unknown` в пересчёте из проекции | 86 817 | подтверждено | S09 |
| 20 | `ensureConflictProjectionForGroup` мутирует группу (кеш в данных); «ensure»-функции v680 меняют фокус/выбор | 86 292, 82 324 | подтверждено | S09, S08 |
| 21 | Проекция v533 включает пути без доказательств владения — у частично гидратированных записей дерево показывает лишние пути | 86 434 | подтверждено (намеренно) | S09 |
| 22 | LG: `priority` композита = min членов (композит проигрывает так, как его самый слабый член); имя цепочки санитизируется по подстроке имени участника | 54 169, 54 222 | вероятно ошибка | S05 |
| 23 | «Разъединить» композит не работает: обработчика нет, `removeOverwriteUserCompositeV657` никто не зовёт | 149 105, 155 132 | вероятно | S17 |
| 24 | `invalidateVisibleMergeProposalControlsV699` не вызывается — устаревшие кнопки «Объединить» до перерисовки (коммит защищён v695) | 153 615 | вероятно | S17 |
| 25 | Таблица контекста использует legacy-фронт/Done, карточки — v665/v667 → разный фокус/Done | 153 257 | подтверждено (частично сглажено v672) | S17 |
| 26 | `loadConflictPresentationEntityDetailsV663` → `null` при неполном покрытии точных цепочек | 150 287 | подтверждено | S17 |
| 27 | `getModPaths`/`readModPaths` при потерянном handle `.mnlth` возвращают `[]` → анализ видит «мод без файлов»; `putModPathsManifestOnly` удаляет метаданные маршрутов; миграция в cold не перечитывает сегмент перед удалением горячей копии | 114 745, 114 363, 128 114 | подтверждено | S12, S14 |
| 28 | `opponentsCount` = Σ ссылок, не уникальных соперников; `overwriteModFiles.priority/enabled` — снимок первого вклада | 119 081, 116 744 | подтверждено | S13 |
| 29 | O(N) IDB-запросов в гидратации (`buildOverwriteStacksForPaths`, участники), O(F²) в `buildParticipantsV501`; два `MutationObserver` на весь документ (зубы, фокус) | 116 417, 124 982, 86 731, 87 879 | подтверждено (производительность) | S13, S14, S09 |
| 30 | Советник: опция `allowCrossSeparator` не используется; «Копия ИИ» исполняет JS через `new Function` с полным `state` | 145 358, 145 622 | подтверждено | S16 |

## 7. Что берёт NG и что меняет

**Берёт (по смыслу, контрактами `sat-core`)**
1. Нормализация пути конфликта (lower-case, `\`→`/`, без `selected/`) и игнор-лист из трёх форм правил → `analysisConfig.conflictIgnorePaths` в каноне проекта; применять в проекции.
2. Победитель = верх стека по живому приоритету; одна шкала приоритета (позиция в эффективном modlist), никаких «stored» значений в данных — стек хранит только множество владельцев.
3. Точная цепочка = стек; контекст = компонента связности. Дополнительно в NG: признак «включён» вне стека (см. п. 6).
4. **Идентичность-истина v502**: id цепочки = хеш множества `(файл, множество владельцев)`, без порядка, победителя и enabled. Это решает нестабильность NG `ChainId`
   и даёт ключ для Done/имён (атомы истины). Отпечаток членства (`owcomptruth`/`owrestruth`) — одна функция над отсортированными атомами.
5. Done и ручные имена — **по множеству атомов** (v665/v666), точное совпадение; частичный Done не продвигается (v502); журнал непрошедших миграцию.
6. Наследование имени контекста — одна метрика (взять Жаккар 0,65/0,30/0,05, порог 0,28, «каждый старый — одному новому»), применять **между пробегами**.
7. Генератор имён/цветов/иконок v459–v468 (двухфазный, детерминированный, OKLab-уникальность, locks, политика `generate/preserve/backfill/regenerate`) — как модуль идентичности.
8. Route groups v499 как **производные предложения**: safe-DAG (общие моды, один победитель, ацикличность, сохранение маршрутов, кап 256/128), цель v500 и формулы ранжирования;
   результат — предложения слияния и композиты пользователя; точный слой неизменяем.
9. Проекция v490: ось файлов, клетки `win|lose|missing|unknown`, «зубы» 1 файл = 1 сегмент — как модель UI (NG-D004); цвета и палитру — для паритета вида.
10. Свежесть по дайджесту порядка (v658) — как инвалидация производных предложений, а не как блок данных.
11. Советник порядка: эвристика score и правила применения (внутри разделителя / новый разделитель), SYNC-запись порядка.

**Меняет**
- IDB-конвейер (256 партиций, work-store, lite-формы, телеметрия, read-views, превью v687, v690 batch) не переносится: память Rust + `.mnlth` (ADR 0007). Переносятся инварианты.
- Слои v498 (археология), v552 entity surface, H-серия холодной БД, самотесты по исходникам — не переносятся.
- Loose > archive — оставить правило NG (ближе к игре), отметить отличие от монолита в паритетных тестах.
- Выключенные моды: решить — NG считает конфликт только среди активных; если нужна «потенциальная» картина монолита (выключенный владелец), добавить второй стек `all_owners`
  с отдельным флагом, но идентичность цепочки строить по активному стеку + множеству всех владельцев (как v502), не по порядку.
- W/L/M/X: развести имена. Мод-уровень NG (`win/lose/mid/fully_overridden/unique`) и цепочка-уровень монолита (`wins/loses/missing/extra`) — разные поля.
- LG-композит — единица участника в проекции; приоритет композита = позиция композита в эффективном списке (не min членов).
- Атомарность: материализация и миграция идентичности — одна транзакция; ошибка не оставляет частичного состояния; статус `failed` у стадий.
- «Копия ИИ» с `new Function` — заменить песочницей расширений; «Разъединить» — явная команда.

## 8. Открытые вопросы

1. Полный путь `rebuildOverwriteConflictModel` → `runOverwriteOwnersStage([])`: срабатывает ли там `modlistToolInvalidateOverwriteNormalizedLayerV501` и последующая материализация (S11 104 274–104 465 vs S17 154 276)?
2. Как часто ветка ошибки №1 достигается на практике: нужен ли старый контекст без совпадения по id/сигнатуре при обычном повторном пробеге (т.е. ломается ли перенос Done при каждом перепробеге с изменениями)?
3. Источник `sourceTruthFingerprints` у safe-групп v499 до v657: строятся ли они для всех групп при materialize или только при наличии `owtruth` у точных цепочек (S14 125 085; S17 открытая нить).
4. Применение патчей `overwrite_user_composites_state` / `overwrite_presentation_identity_state` в `applyAutosavePatch` и попадание `ui.overwrite*V65x/V66x` в light-снимок.
5. Где пишется `state.conflictsUi.*` (режим модалки, `cardsShowResolved`, `presentationSelectionV662`) в сохранение проекта.
6. Какая доля цепочек в реальной сборке (~2 ТБ) получает конфликт только из-за выключенного владельца (решение по п. 2 §5).
7. Сверить CP10-финализатор (172 897, тень) с живым IDB-конвейером: он строит контексты union-find и группы по `routeSignature` — расходится ли в счётчиках (cutover отключил его: `streamingAlternativeDisabled`).
8. Вызывающие `getOverwriteRelationCountsForModsV645` (102 393) и итоговые счётчики бейджей «цепочек/контекстов» при presentation-режиме.
9. Поведение `getLogicalGroupNGAuthoritativeConflictGroupsForModV571 → null` у всех потребителей обл. 8 (ветки наследного пути 81 105–82 300).
