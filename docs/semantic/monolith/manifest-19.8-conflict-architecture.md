# MODEL-OPERATING MANIFEST
## VIBE MONOLITH · SEMANTIC DISTANCE MODE
### Architectural Addendum · 19.8. Unified Conflict Architecture

Этот файл является постоянным архитектурным дополнением к актуальному `MODEL-OPERATING MANIFEST.md`.

Вставлять как подраздел **19.8** после `19.7. UI contexts` внутри раздела `19. ГЛОБАЛЬНЫЕ АРХИТЕКТУРНЫЕ НАПРАВЛЕНИЯ`.

---

## 19.8. Единая архитектура конфликтов

Монолит уже умеет обнаруживать и разбирать файловые overwrite-конфликты. Не считать текущую файловую модель исчерпывающим определением конфликта.

На уровне продукта и пользовательского языка стремиться к единому понятию:

```text
Конфликты
```

При этом внутри архитектуры не сводить все конфликты к модели:

```text
несколько модов предоставляют один файл → последний победил
```

И не считать порядок загрузки универсальным resolver-механизмом.

Обобщающий принцип:

> Конфликт — ситуация, в которой два или более участника воздействуют на общий объект, область, состояние или ограничение, а их совместный результат неоднозначен, нежелателен либо невозможен.

Универсальной осью является не сам `load order`, а **precedence / priority / resolution semantics** конкретного конфликтного домена.

### 19.8.1. Основные семейства конфликтов

Различать как минимум четыре фундаментальных семейства.

#### Collision

Участники претендуют на один идентичный target:

```text
same virtual path
same FormID
same config key
same runtime hook site
```

Для такого конфликта может существовать winner.

#### Merge conflict

Участники изменяют разные части общего target и правильный результат требует композиции, а не выбора одного победителя.

Пример:

```text
Mod A → AI packages
Mod B → appearance
Mod C → inventory

Desired result = A.AI + B.Appearance + C.Inventory
```

Не пытаться искусственно превращать merge conflict в обычный last-wins overwrite.

#### Interaction conflict

Targets формально различны, но эффекты пересекаются.

Примеры:

```text
два independently-added объекта занимают одну область мира
два scripts управляют одним runtime entity
две animation/runtime conditions срабатывают одновременно
```

Такой конфликт может не иметь общего virtual path или FormID.

#### Constraint / compatibility conflict

Совместная конфигурация нарушает ограничение или вообще невозможна.

Примеры:

```text
missing master
wrong runtime version
incompatible frameworks
stale generated output
unsatisfied dependency
mutually exclusive components
```

У такого конфликта winner может отсутствовать принципиально.

### 19.8.2. Домены конфликтов

Во время прохода монолита учитывать возможность существования следующих типов targets и analyzers.

#### File conflict

```text
target = VirtualPath
policy = LAST_WINS
precedenceDomain = MOD_ORDER
```

Это текущий наиболее зрелый конфликтный домен монолита.

Существующие overwrite chains / contexts / winners не выбрасывать и не переписывать только ради новой абстракции. Рассматривать их как первый доказанный adapter общего conflict layer.

Целевая семантика:

```text
FileConflictAnalyzer
    ↓
ConflictCase {
    family: FILE_COLLISION,
    target: VirtualPath,
    interactionPolicy: LAST_WINS,
    precedenceDomain: MOD_ORDER
}
```

#### Plugin record conflict

```text
target = FormID / record
precedenceDomain = PLUGIN_LOAD_ORDER
resolution = reorder OR patch
```

Учитывать, что последний record winner не гарантирует желаемого объединения изменений.

#### Record field / subrecord conflict

```text
target = field/subrecord inside FormID
resolution = field provenance + merge/patch
```

Не считать весь record атомарным, если полезная диагностика требует видеть происхождение отдельных полей.

#### Collection / merge conflict

Примеры:

```text
leveled lists
FormLists
keyword-like collections
other mergeable record structures
```

```text
interactionPolicy = MERGEABLE
resolution = union / keyed merge / generated patch / manual patch
```

Не требовать одного победителя, когда корректный результат является композицией.

#### Spatial conflict

```text
target = cell / worldspace region / spatial volume
interactionPolicy = simultaneous composition
```

Примеры:

- пересекающиеся объекты;
- перекрытые двери и проходы;
- landscape incompatibility;
- water/lighting interaction;
- road seams;
- worldspace placement conflicts.

Такой конфликт может существовать без record overwrite.

#### Navmesh conflict

```text
target = navigation graph / navmesh boundary
resolution = graph reconciliation / dedicated patch
```

Не предполагать, что plugin order способен разрешить геометрически или топологически несовместимые navmesh.

#### Dependency / compatibility conflict

```text
target = dependency/constraint graph
interactionPolicy = CONSTRAINT_BASED
```

Resolution может включать:

```text
install requirement
update/downgrade
reorder
replace
remove/disable participant
install compatibility patch
```

#### Generated-output conflict

Generated artifacts являются функцией входной конфигурации:

```text
Output = Generate(mod order, plugin order, configs, tool version, inputs...)
```

Учитывать stale provenance.

Типичные источники:

```text
DynDOLOD / TexGen
xLODGen
Synthesis
Pandora / Nemesis / FNIS
BodySlide
Grass cache
Occlusion
other patchers/generators
```

```text
target = DerivedArtifactDomain
interactionPolicy = GENERATED
resolution = REGENERATE_OUTPUT
```

Изменение load order здесь может быть входом генерации, но не непосредственным resolver-механизмом.

#### Animation / behavior framework conflict

Учитывать независимые framework-specific priority domains:

```text
Pandora/Nemesis patch priority
OAR/DAR selection priority
conditions
behavior graph ownership
```

```text
target = animation event / behavior / runtime condition
resolution = priority adjustment / condition narrowing / regeneration / compatibility patch
```

#### Runtime rule / distributor conflict

Для систем наподобие:

```text
SPID
KID
CID
BOS
OAR conditions
SkyPatcher
other INI/JSON distributors
```

два правила могут воздействовать на один runtime entity, не перезаписывая один физический файл и не редактируя один FormID.

```text
target = runtime entity + property
interactionPolicy = additive / replacement / framework-specific
```

Resolution может требовать изменения filters, conditions, priority или самих правил.

#### Runtime DLL / hook conflict

```text
target = runtime ABI / engine function / hook site
interactionPolicy = EXCLUSIVE or framework-specific
```

Учитывать:

- несовместимую runtime/SKSE version;
- конкурирующие hooks;
- несовместимые Address Library/API expectations;
- mismatched DLL/scripts versions.

Порядок файлов разрешает только физический overwrite одинакового пути и не обязан разрешать два разных DLL, патчащих один runtime target.

#### Papyrus runtime conflict

Различать:

```text
same .pex path → file conflict
```

и runtime interaction между разными scripts:

```text
shared quest ownership
shared events
shared globals
shared hotkeys
parallel NPC/state mutation
persistent script state
```

```text
target = runtime state/event/quest ownership
```

#### Asset contract conflict

Формально корректные winners отдельных файлов могут составить внутренне несовместимый набор.

Пример:

```text
Mesh      ← Mod A
Skeleton  ← Mod B
Animation ← Mod C
Textures  ← Mod D
```

Каждый путь отдельно разрешён, но связанный asset bundle может нарушить контракт:

- отсутствующая bone;
- неверный texture/material path;
- несовместимые morphs;
- body topology mismatch;
- incompatible animation graph.

```text
target = AssetBundleContract
interactionPolicy = UNRESOLVED_SEMANTIC
```

Это межфайловый семантический конфликт, который не обязан быть видимым в анализе одного virtual path.

#### Save-state / lifecycle conflict

```text
target = persisted game state
participants = historical mod state + current installation
```

Учитывать:

- active quests;
- persistent references;
- serialized SKSE data;
- scripts/events;
- aliases;
- FormID lifecycle;
- incompatible mod removal/update.

Resolution может требовать migration, restore old version, controlled cleanup или new game. Load order не является универсальным resolver.

### 19.8.3. Precedence domains

Не строить архитектуру вокруг одного общего `loadOrder`.

Возможны независимые precedence domains:

```text
MOD_ORDER
PLUGIN_LOAD_ORDER
RECORD_FIELD_PROVENANCE
PATCH_PIPELINE_ORDER
GENERATOR_EXECUTION_ORDER
FRAMEWORK_PRIORITY
RUNTIME_INITIALIZATION_ORDER
VERSION_DEPENDENCY_PRECEDENCE
NONE / NOT_APPLICABLE
```

Один `ConflictCase` должен явно знать, какой precedence domain релевантен ему, если такой domain вообще существует.

Не переносить правила одного domain на другой автоматически.

### 19.8.4. Общая модель ConflictCase

При появлении или рефакторинге конфликтных подсистем стремиться к общей семантической сущности примерно следующего уровня:

```text
ConflictCase
  id
  family
  target
  participants[]
  contexts[]
  evidence[]
  precedenceDomain
  interactionPolicy
  currentOutcome
  desiredOutcome
  availableResolutions[]
  severity
  confidence
  provenance
  freshness
```

Это направление, а не требование немедленно переписать текущий код под заранее фиксированную schema.

Сначала устанавливай реальные данные, consumers и authority существующих конфликтных подсистем.

### 19.8.5. Типизированные targets

Архитектура должна допускать расширение targets без превращения каждого нового вида конфликта в файловую overwrite-chain.

Возможные target types:

```text
VirtualPathTarget
PluginRecordTarget
RecordFieldTarget
CollectionTarget
SpatialRegionTarget
NavmeshTarget
DependencyTarget
RuntimeHookTarget
RuntimeRuleTarget
GeneratedArtifactTarget
PersistentStateTarget
AssetContractTarget
```

Не считать этот перечень закрытым.

### 19.8.6. Interaction policies

Conflict engine должен допускать разные семантики взаимодействия:

```text
LAST_WINS
FIRST_WINS
MERGEABLE
ADDITIVE
EXCLUSIVE
CONDITIONAL
GENERATED
CONSTRAINT_BASED
UNRESOLVED_SEMANTIC
```

Не вводить fictitious winner, если фактическая семантика конфликта его не имеет.

### 19.8.7. Resolver-механизмы

Доступные способы разрешения также типизированы и не сводятся к reorder:

```text
REORDER_MODS
REORDER_PLUGINS
CREATE_PATCH
MERGE_VALUES
CONFIGURE_RULES
REGENERATE_OUTPUT
INSTALL_REQUIREMENT
CHANGE_VERSION
DISABLE_PARTICIPANT
REPLACE_PARTICIPANT
MANUAL_EDIT
MIGRATE_STATE
START_NEW_STATE
ACCEPT_INTENTIONAL_CONFLICT
```

Не выполнять потенциально разрушительное разрешение автоматически только потому, что analyzer способен его предложить.

Разделять:

```text
detection
explanation
preview
recommendation
execution
validation
```

### 19.8.8. Продуктовая семантика

Для пользователя предпочтительно единое рабочее пространство **«Конфликты»** вместо разрозненных несвязанных инструментов для каждого домена.

Но единый UI не означает единую внутреннюю механику.

Стремиться к модели:

```text
Конфликты
  ├─ Files
  ├─ Plugin records
  ├─ Fields / merges
  ├─ Spatial / navmesh
  ├─ Dependencies
  ├─ Generated outputs
  ├─ Runtime/framework rules
  ├─ DLL/hooks
  ├─ Asset contracts
  └─ Save/lifecycle
```

Конкретная taxonomy UI может меняться по фактическому UX и данным. Не превращать этот список в заранее запечатанную навигацию.

### 19.8.9. Отношение к текущим overwrite chains и contexts

Текущие overwrite chains, contexts, badges, winners и conflict inspection являются ценным существующим доменом.

Не ломать их ради абстракции.

Во время рефакторинга:

- сохранить наблюдаемую правдивость файловых конфликтов;
- установить их canonical data owners;
- отделить file-specific semantics от общего conflict shell там, где это оправдано;
- позволить существующему файловому анализатору стать adapter/provider общего conflict layer;
- не заставлять нефайловые конфликты имитировать `overwrite chain`;
- не заставлять merge/interaction/constraint conflicts иметь фиктивного winner;
- сохранять provenance и evidence каждого вывода;
- различать detected conflict, intentional conflict и unresolved conflict.

### 19.8.10. Операционный принцип для прохода монолита

Когда во время основного прохода встречается код, связанный с конфликтами, задавай не только вопрос:

```text
кто кого перезаписывает?
```

а полный набор вопросов:

```text
Каков target?
Кто participants?
Какова interaction semantics?
Есть ли precedence domain?
Как формируется current outcome?
Есть ли один winner вообще?
Какой resolver реально способен изменить outcome?
Нужен reorder, merge, patch, regeneration, configuration или compatibility action?
Каковы evidence, provenance, confidence и freshness?
Как этот conflict должен жить в общем ConflictCase без потери domain-specific данных?
```

Не активировать немедленную большую миграцию только потому, что общая модель стала понятна.

Использовать это направление по мере фактического прохода и появления подходящих architectural seams.

Главная формула:

```text
Conflict
  → target
  → participants
  → interaction semantics
  → precedence domain
  → current outcome
  → resolution policy
```

а не:

```text
Conflict → load order
```

Итоговая архитектурная цель:

> Единое понятие «Конфликты» на уровне продукта и общего semantic layer; типизированные targets, precedence domains и resolver-механизмы внутри; текущий файловый overwrite-анализ сохраняется и становится первым зрелым adapter общего conflict architecture.
