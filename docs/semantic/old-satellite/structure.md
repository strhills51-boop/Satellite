# Старый Satellite — структура (автоматически)

Файл: `preview_40_file_windows_context_fix.html` (21 883 строки; CSS до 3 515, скрипт 3 515–21 883). Разделы — по маркерам `@sat:`. Формат: строки [длина] раздел, объекты, методы.

```
3526-3538 [13] constants  objs=Satellite.constants  regs=0
   methods: 
3539-3584 [46] utils  objs=Satellite.utils  regs=0
   methods: for
3585-3622 [38] identity  objs=Satellite.identity  regs=0
   methods: createId displayKey sourceRef ensureAliasBucket addAlias
3623-3719 [97] events  objs=Satellite.events  regs=0
   methods: on once off emit for record if
3720-3948 [229] registry  objs=Satellite.registry  regs=0
   methods: registerPrimitive registerWrapper registerValidator registerImporter registerRenderer registerCommand registerCommitAction registerHotkey registerUiVariant registerEventHandler registerJob registerExtensionManifest registerResourceContract registerRuntimeApi registerRunContract registerBarrierResolver registerPreset getPrimitive getWrapper getValidator getImporter getRenderer getComman
3949-4176 [228] resource-contracts  objs=Satellite.resourceContracts  regs=0
   methods: normalizeSource trustLevelForSource ensureId if normalizeCapabilities normalizeLifecycle normalizeWrapper validateWrapper normalizeRenderer normalizeCommand normalizeJob normalizeHotkey normalizeUiVariant
4177-4376 [200] resource-policy  objs=Satellite.resourcePolicy  regs=0
   methods: defaultPolicy resourceId trustLevel isTrusted canUseResource if guard canUseRuntimeApi assertRuntimeApi canBindWrapper canUseRenderer canRunLifecycle canRunCommand canUseHotkey canUseUiVariant canStartJob
4377-4520 [144] runtime-api  objs=Satellite.runtimeApi  regs=0
   methods: clone resourceMeta createForResource
4521-4691 [171] extension-loader  objs=Satellite.extensionLoader  regs=12
   methods: normalizeManifest if for registerManifest registerManifestResources resolveManifestIdsFromPresetStack collectActiveResources bindProjectExtensions
4692-4804 [113] resource-composer  objs=Satellite.resourceComposer  regs=0
   methods: normalizePresetStack resolveStartupWrapperIds resolveUiWrapperIds resolveCommandWrapperIds collectProjectValidatorIds collectPresetHotkeyIds buildProjectResources
4805-5529 [725] resource-center  objs=Satellite.resourceCenter  regs=0
   methods: mergeUnique for registrationKindForId isActiveNode findManifestIdsForResource findPresetIdsForResource linksForResource if diagnoseResource openModal renderStatusChips getWindowUsedResourceIds renderLinkedResourceList renderWindowUsageBlock renderResourceDetails renderModalBody bindModal
5530-5788 [259] resource-runtime  objs=Satellite.resourceRuntime  regs=0
   methods: bindProjectResources bindWindow if resolveWindowWrapper getWindowCapabilities renderWindowBody isWindowVirtualized getProjectionScopesForWindow unmountWindow handleInvalidation for callLifecycleHook
5789-5968 [180] registry.primitives  objs=  regs=13
   methods: render open close renderCard
5969-6031 [63] ui-dots  objs=Satellite.uiDots  regs=1
   methods: resolveVariant render
6032-6120 [89] hotkeys  objs=Satellite.hotkeys  regs=0
   methods: normalizeCombo isEditableTarget contextFor register listActive scopeMatches handleKeydown if
6121-6285 [165] command-runtime  objs=Satellite.commandRuntime  regs=0
   methods: context dispatch if applyDeclaredInvalidation record
6286-6552 [267] jobs  objs=Satellite.jobs  regs=1
   methods: ensureRuntime start if pump while run createContext setProgress cancel finishCancelled fail complete publicPacket
6553-6632 [80] modal  objs=Satellite.modal,Satellite.dom.modalRoot.className,Satellite.dom.modalRoot.innerHTML,Satellite.dom.modalBackdrop.onclick,Satellite.confirmModal  regs=0
   methods: open close
6633-6715 [83] context-menu  objs=Satellite.contextMenu  regs=0
   methods: open close
6716-6786 [71] header-menu  objs=Satellite.headerMenu  regs=0
   methods: bind runFileAction if refreshWindowList
6787-7140 [354] window-library  objs=Satellite.windowLibrary  regs=0
   methods: inferWindowType listAvailableWindowDefinitions if filteredDefinitions open renderBody bind refresh renderPreview ensureParamsForWrapper renderParameterBlock bindPreviewParams defaultRectForIndex createWindow applySelection
7141-12071 [4931] run-engine  objs=Satellite.runEngine  regs=0
   methods: getStageWrapper listContractsForStage resolveContract resolveBarrierResolver normalizeBarrierRequest rebuildBarrierDictionary ensureBarrierDictionary matchBarrierDictionary createBarrierInstruction listBarrierResolverContracts detectAccessBarrier requestAccess if resolveBarrier ensureContractsRegistered makeStage isStageDefaultEnabled buildPlan createSession getSession createRouteStore
12072-12762 [691] project-resources  objs=Satellite.projectResources  regs=0
   methods: slug buildAiContext copyText createUiDsl renderUiNode createExtensionContext createFactoryScope runExtensionFactory normalizeExtensionSpec if previewExtension makeWindowResourcesFromExtension for normalizeInjectedSpec makeWindowResource registerResource registerWindowResource renderTemplate renderProjectWindowResource
12763-13123 [361] developer-center  objs=Satellite.developerCenter  regs=0
   methods: render renderForm renderDetails if bind readDraft saveWindowResource previewDraft copyAiContext validateExtensionFactory injectResourceFactory openModal openWindow
13124-13127 [4] registry.validators.core  objs=  regs=0
   methods: 
13128-13146 [19] registry.validators.core  objs=  regs=1
   methods: validate
13147-13213 [67] registry.validators.mo2  objs=  regs=4
   methods: validate validateProject if
13214-13254 [41] registry.importers.mo2  objs=  regs=3
   methods: import
13255-13578 [324] file-windows-v1-reference  objs=Satellite.fileWindows  regs=0
   methods: e if setSelectedLeaf getRouteStore makeFileRecord getAllFileRecords for loadRouteIndexFromIndexedDb buildTree flattenTree renderFileTree renderFileManager renderIgnoredFiles renderSameExtension ensureGlobalHandlers bindCommon setHeadMeta
13579-14015 [437] conflict-windows-v1-port  objs=Satellite.conflictWindows  regs=0
   methods: e getAnalysis leafFileMap getContextForChain getSelectedContext setSelectedChain toggleDoneChain openToothWindow renderChainsWindow if renderContextBlock buildMiniTreeHtml buildChainModListHtml for renderChainCard bindChainsWindow renderToothWindow buildToothView renderStackMark renderToothSection bindToothWindow
14016-14110 [95] registry.renderers.core  objs=  regs=11
   methods: renderBody
14111-14443 [333] registry.wrappers.core  objs=  regs=23
   methods: mount invalidate update render
14444-14501 [58] registry.wrappers.mo2  objs=  regs=3
   methods: 
14502-14799 [298] registry.commands.core  objs=  regs=16
   methods: execute canRun if run
14800-14880 [81] extension-manifests.core  objs=  regs=2
   methods: 
14881-15005 [125] registry.presets  objs=  regs=6
   methods: 
15006-15148 [143] schema  objs=Satellite.schema  regs=0
   methods: 
15149-15243 [95] schema.ui  objs=  regs=0
   methods: 
15244-15298 [55] store  objs=Satellite.store  regs=0
   methods: 
15299-15434 [136] lifecycle  objs=Satellite.lifecycle,Satellite.store.state.runtime,Satellite.store.state.runtime.phase,Satellite.store.state.runtime.loadedAt,Satellite.store.state.runtime.lastReason,Satellite.store.state.runtime.projections  regs=0
   methods: 
15435-15614 [180] project-flow  objs=Satellite.projectFlow,Satellite.store.state.project  regs=0
   methods: 
15615-15809 [195] architecture-map  objs=Satellite.architecture  regs=0
   methods: 
15810-16041 [232] architecture-guard  objs=Satellite.architectureGuard  regs=0
   methods: 
16042-16505 [464] migrations  objs=Satellite.migrations  regs=0
   methods: 
16506-16642 [137] normalize  objs=Satellite.normalize  regs=0
   methods: 
16643-16790 [148] entities  objs=Satellite.entities  regs=0
   methods: 
16791-16860 [70] rows  objs=Satellite.rows  regs=0
   methods: 
16861-16908 [48] indexes  objs=Satellite.indexes  regs=0
   methods: 
16909-17125 [217] importers  objs=Satellite.importers  regs=0
   methods: 
17126-17245 [120] selectors  objs=Satellite.selectors  regs=0
   methods: 
17246-17337 [92] invalidator  objs=Satellite.invalidator  regs=0
   methods: 
17338-17377 [40] scheduler  objs=Satellite.scheduler  regs=0
   methods: 
17378-17546 [169] projections  objs=Satellite.projections  regs=0
   methods: 
17547-17693 [147] virtualizer  objs=Satellite.virtualizer  regs=0
   methods: 
17694-17900 [207] mutations  objs=Satellite.mutations  regs=0
   methods: 
17901-18066 [166] commands  objs=Satellite.commandHandlers,Satellite.commands  regs=0
   methods: 
18067-18128 [62] file-io  objs=Satellite.fileIO,Satellite.dom.fileInput.value,Satellite.dom.fileInput.accept  regs=0
   methods: 
18129-18634 [506] start-wizard  objs=Satellite.startWizard,Satellite.dom.startWizardRoot.innerHTML,Satellite.dom.fileInput.value,Satellite.dom.fileInput.accept  regs=0
   methods: open close render renderModeCard renderModePane renderContinuePane renderLoadPane renderNewProjectPane renderStepBadge renderNewProjectStep renderProjectNameStep renderPresetSelectionStep renderPresetCard renderSourcesStep if renderValidationMessages renderStartStatusText bindRenderedControls togglePreset canGoPrevStep canGoNextStep goPrevStep goNextStep isProjectNameValid resolveStart
18635-18750 [116] scroll-state  objs=Satellite.scrollState  regs=0
   methods: keyFor if nodes capture restore transaction bindGlobal
18751-18815 [65] window-factory  objs=Satellite.windowFactory  regs=0
   methods: create if
18816-18837 [22] render  objs=Satellite.render,Satellite.dom.status.textContent  regs=0
   methods: 
18838-18941 [104] render.workspace  objs=Satellite.dom.workspace.innerHTML  regs=0
   methods: if for
18942-18975 [34] render.window-shell  objs=  regs=0
   methods: 
18976-19171 [196] render.modlist-window  objs=  regs=0
   methods: if
19172-19249 [78] render.mod-row  objs=  regs=0
   methods: 
19250-19380 [131] render.meta-window  objs=  regs=0
   methods: if
19381-19816 [436] diagnostics  objs=Satellite.diagnostics  regs=0
   methods: open bind rerenderInspector renderBody inspectKinds getInspectableItems if renderInspector describeResource describeWindow describeMod snapshot
19817-19837 [21] window-manager  objs=Satellite.windowManager  regs=0
   methods: 
19838-19873 [36] window-manager.delegation  objs=  regs=0
   methods: bind
19874-19957 [84] window-manager.click  objs=  regs=0
   methods: handleClick if handleDoubleClick
19958-20314 [357] window-manager.pointer-down  objs=  regs=0
   methods: if startModDrag findModDropCandidate for ensureModDropMarker showModDropMarker hideModDropMarker workspacePointFromEvent openWorkspaceContextMenu handleContextMenu focus applyWindowZ
20315-20370 [56] window-manager.drag-start  objs=  regs=0
   methods: startDrag if
20371-20484 [114] window-manager.drag-move  objs=  regs=0
   methods: if
20485-20505 [21] window-manager.drag-end  objs=  regs=0
   methods: 
20506-20526 [21] window-manager.resize-start  objs=  regs=0
   methods: startResize
20527-20578 [52] window-manager.resize-move  objs=  regs=0
   methods: if
20579-21446 [868] window-manager.viewport  objs=  regs=0
   methods: if findDockCandidate for getDockZone getDockDepth computeDockSector makeDockLabel pointInRect commitDock detachWindow captureGroupRects resizeGroupFromWindow applyWindowRect showPreview hidePreview
21447-21884 [438] window-manager.pan  objs=Satellite.dom,Satellite.boot  regs=0
   methods: startPan for if
```
