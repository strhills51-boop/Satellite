<script lang="ts">
  import { api } from '../lib/api';
  import { app, type RightTab } from '../lib/store.svelte';
  import SummaryTab from './SummaryTab.svelte';
  import ConflictsTab from './ConflictsTab.svelte';
  import TreeTab from './TreeTab.svelte';
  import PluginsTab from './PluginsTab.svelte';
  import ContextsTab from './ContextsTab.svelte';
  import PathTable from './PathTable.svelte';

  const tabs: [RightTab, string][] = [
    ['summary', 'Сводка'],
    ['conflicts', 'Конфликты'],
    ['tree', 'Дерево'],
    ['contexts', 'Контексты'],
    ['plugins', 'Плагины'],
    ['files', 'Файлы'],
  ];
</script>

<div class="panel last">
  <div class="panel-header">
    Сводка <span class="muted small ellipsis">{app.focus || 'сборка'}</span>
    <span class="grow"></span>
    {#if app.focus}<button class="btn ghost sm" title="Показать сводку сборки" onclick={() => { app.focus = ''; app.selection.clear(); app.rightTab = 'summary'; }}>×</button>{/if}
  </div>
  <div class="tabs">
    {#each tabs as [id, label]}
      <button class="tab" class:active={app.rightTab === id} onclick={() => (app.rightTab = id)}>{label}</button>
    {/each}
  </div>
  <div class="content">
    {#if app.rightTab === 'summary'}<SummaryTab />
    {:else if app.rightTab === 'conflicts'}<ConflictsTab />
    {:else if app.rightTab === 'tree'}<TreeTab />
    {:else if app.rightTab === 'contexts'}<ContextsTab />
    {:else if app.rightTab === 'plugins'}<PluginsTab />
    {:else if app.rightTab === 'files'}
      {#if app.view?.analyzed}
        <PathTable placeholder="Путь в виртуальной Data (мин. 2 символа)…" load={(f, o) => api.search(f, o)} />
      {:else}<div class="empty">Поиск по файлам доступен после анализа.</div>{/if}
    {/if}
  </div>
</div>

<style>
  .last { border-right: 0; }
  .content { flex: 1 1 auto; min-height: 0; overflow: auto; }
</style>
