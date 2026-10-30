<script lang="ts">
  import { onMount } from 'svelte';
  import { api, type JournalEntry } from './lib/api';
  import { app } from './lib/store.svelte';
  import Separators from './components/Separators.svelte';
  import ModList from './components/ModList.svelte';
  import RightPanel from './components/RightPanel.svelte';
  import StartScreen from './components/StartScreen.svelte';
  import RunModal from './components/RunModal.svelte';
  import ContextMenu from './components/ContextMenu.svelte';

  let fileMenu = $state(false);
  let historyOpen = $state(false);
  let journal = $state<JournalEntry[]>([]);
  let left = $state(240);
  let right = $state(520);

  onMount(() => {
    try {
      const s = JSON.parse(localStorage.getItem('mlt.panes') || '{}');
      if (s.left) left = s.left;
      if (s.right) right = s.right;
      if (s.viewMode) app.viewMode = s.viewMode;
    } catch { /* storage unavailable */ }
    app.init();
  });
  $effect(() => {
    const v = { left, right, viewMode: app.viewMode };
    try { localStorage.setItem('mlt.panes', JSON.stringify(v)); } catch { /* ignore */ }
  });

  function key(e: KeyboardEvent) {
    const t = e.target as HTMLElement;
    const typing = t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA');
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 's') { e.preventDefault(); app.save(); }
    else if (mod && e.key === '2') { e.preventDefault(); app.viewMode = app.viewMode === 'classic' ? 'explorer' : 'classic'; }
    else if (!typing && mod && e.key.toLowerCase() === 'z' && !e.shiftKey) { e.preventDefault(); app.undo(); }
    else if (!typing && mod && (e.key.toLowerCase() === 'y' || (e.key.toLowerCase() === 'z' && e.shiftKey))) { e.preventDefault(); app.redo(); }
    else if (e.key === 'Escape' && app.modal === 'run' && !app.run?.active) app.modal = '';
  }

  function gutter(e: PointerEvent, which: 'l' | 'r') {
    const x0 = e.clientX;
    const l0 = left;
    const r0 = right;
    const move = (ev: PointerEvent) => {
      if (which === 'l') left = Math.max(160, Math.min(520, l0 + ev.clientX - x0));
      else right = Math.max(320, Math.min(window.innerWidth - left - 320, r0 - (ev.clientX - x0)));
    };
    const up = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', up);
  }

  async function openHistory() {
    historyOpen = !historyOpen;
    if (historyOpen) journal = (await api.journal().catch(() => [])) ?? [];
  }
  const runActive = $derived(!!app.run?.active);
  const activeStage = $derived(app.run?.stages.find((s) => s.status === 'active'));
</script>

<svelte:window onkeydown={key} onclick={(e) => { if (!(e.target as HTMLElement).closest('.menu-anchor')) { fileMenu = false; historyOpen = false; } }} />

<div class="app">
  <div class="toolbar">
    <div class="title"><span class="tmain">ModlistTool.mo2</span> <span class="muted">| Monolith</span></div>
    <div class="menu-anchor">
      <button class="btn primary" onclick={() => (fileMenu = !fileMenu)}>Файл</button>
      {#if fileMenu}
        <div class="dropdown">
          <button class="dd" onclick={() => { fileMenu = false; app.modal = 'start'; }}>Открыть инстанс / профиль…</button>
          <button class="dd" disabled={!app.view?.dirty} onclick={() => { fileMenu = false; app.save(); }}>Сохранить modlist.txt <span class="muted">Ctrl+S</span></button>
          <div class="dvd"></div>
          <button class="dd" disabled={!app.view?.canUndo} onclick={() => { fileMenu = false; app.undo(); }}>Отменить <span class="muted">Ctrl+Z</span></button>
          <button class="dd" disabled={!app.view?.canRedo} onclick={() => { fileMenu = false; app.redo(); }}>Повторить <span class="muted">Ctrl+Y</span></button>
          <div class="dvd"></div>
          <button class="dd" onclick={() => { fileMenu = false; app.reload(); }}>Обновить вид</button>
        </div>
      {/if}
    </div>
    <button class="btn" class:running={runActive} disabled={!app.view?.instance} onclick={() => (app.modal = 'run')}>
      {runActive ? `◐ ${activeStage ? Math.round(activeStage.percent) + '%' : 'Анализ…'}` : 'Анализ mods'}
    </button>
    <button class="btn" onclick={() => (app.viewMode = app.viewMode === 'classic' ? 'explorer' : 'classic')} title="Ctrl+2">
      {app.viewMode === 'classic' ? 'Вид: classic' : 'Вид: explorer'}
    </button>
    <div class="menu-anchor">
      <button class="btn" onclick={openHistory}>История действий</button>
      {#if historyOpen}
        <div class="dropdown hist">
          {#each journal.slice(0, 40) as h}
            <div class="hrow"><b>{h.type}</b> {h.text}<div class="muted small">{new Date(h.at).toLocaleString()}</div></div>
          {:else}<div class="muted hrow">История пуста</div>{/each}
        </div>
      {/if}
    </div>
    <div class="toolbar-right">
      {#if app.view?.dirty}
        <button class="btn sm dirty" onclick={() => app.save()} title="Порядок изменён — Ctrl+S запишет modlist.txt (с бэкапом)">● не сохранено</button>
      {/if}
      <div class="status">
        {#if app.view?.instance}
          {app.view.instance.gameName || 'MO2'} · {app.view.profile} · {app.summary?.Enabled ?? 0}/{app.summary?.Mods ?? 0} модов
          {#if app.view.analyzed} · {app.summary?.ConflictPaths.toLocaleString('ru-RU')} конфликтов{:else} · анализ не выполнен{/if}
        {:else}Нет проекта{/if}
      </div>
    </div>
  </div>

  <div class="view-root">
    {#if app.view?.instance}
      <div class="classic" style="grid-template-columns: {app.viewMode === 'classic' ? left + 'px 6px' : ''} 1fr 6px {right}px">
        {#if app.viewMode === 'classic'}
          <Separators />
          <div class="gutter" role="separator" onpointerdown={(e) => gutter(e, 'l')}></div>
        {/if}
        <ModList />
        <div class="gutter" role="separator" onpointerdown={(e) => gutter(e, 'r')}></div>
        <RightPanel />
      </div>
    {:else}
      <div class="empty" style="padding-top:20vh">
        <button class="btn primary" onclick={() => (app.modal = 'start')}>Открыть инстанс MO2</button>
      </div>
    {/if}
  </div>
</div>

{#if app.modal === 'start'}<StartScreen />{/if}
{#if app.modal === 'run'}<RunModal />{/if}
<ContextMenu />
{#if app.toast}<div class="toast {app.toast.kind}">{app.toast.text}</div>{/if}

<style>
  .app { height: 100vh; display: flex; flex-direction: column; }
  .toolbar {
    height: var(--header-h); min-height: var(--header-h); display: flex; align-items: center; gap: 8px; padding: 8px 10px;
    background: rgba(18, 24, 32, .94); border-bottom: 1px solid var(--line); z-index: 50;
  }
  .title { font-weight: 800; white-space: nowrap; padding-right: 6px; }
  .tmain { background: linear-gradient(90deg, #9ac6ff, #65a8ff); -webkit-background-clip: text; background-clip: text; color: transparent; }
  .toolbar-right { margin-left: auto; display: flex; align-items: center; gap: 8px; min-width: 0; }
  .status { color: var(--muted); font-size: 12px; max-width: 40vw; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .menu-anchor { position: relative; }
  .dropdown {
    position: absolute; top: calc(100% + 4px); left: 0; min-width: 260px; border-radius: 12px; border: 1px solid var(--line);
    background: var(--pop); box-shadow: var(--shadow); padding: 6px; z-index: 80; display: flex; flex-direction: column;
  }
  .dd { display: flex; justify-content: space-between; gap: 12px; text-align: left; padding: 7px 10px; border: 0; background: transparent; border-radius: 8px; cursor: pointer; }
  .dd:hover:not(:disabled) { background: #2a3647; }
  .dd:disabled { opacity: .4; cursor: default; }
  .dvd { height: 1px; background: var(--line); margin: 4px; }
  .hist { width: 420px; max-height: 60vh; overflow: auto; }
  .hrow { padding: 6px 8px; border-bottom: 1px solid rgba(52,65,85,.5); font-size: 12.5px; }
  .dirty { border-color: #a16207; background: #422006; color: #fcd34d; }
  .running { border-color: var(--accent); }
  .view-root { flex: 1; min-height: 0; position: relative; }
  .classic { height: 100%; min-height: 0; display: grid; }
  .gutter { cursor: col-resize; background: transparent; }
  .gutter:hover { background: rgba(101,168,255,.25); }
  .toast {
    position: fixed; bottom: 16px; left: 50%; transform: translateX(-50%); z-index: 500; padding: 9px 14px; border-radius: 12px;
    border: 1px solid var(--line); background: var(--pop); box-shadow: var(--shadow); max-width: 70vw;
  }
  .toast.ok { border-color: rgba(34,197,94,.5); }
  .toast.err { border-color: rgba(239,68,68,.6); color: #fecaca; }
</style>
