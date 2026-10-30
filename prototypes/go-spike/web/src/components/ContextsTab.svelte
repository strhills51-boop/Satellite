<script lang="ts">
  import { api, type ContextView } from '../lib/api';
  import { app, colorToBg } from '../lib/store.svelte';
  import PathTable from './PathTable.svelte';

  let list = $state<ContextView[]>([]);
  let cur = $state<ContextView | null>(null);
  let chainId = $state(-1);
  let hideDone = $state(false);

  $effect(() => {
    void app.gen;
    api.contexts().then((x) => (list = x ?? [])).catch(() => (list = []));
  });
  $effect(() => {
    const id = app.focusContext;
    void app.gen;
    if (id < 0) { cur = null; return; }
    api.context(id).then((c) => { cur = c; chainId = c.chains[0]?.id ?? -1; }).catch(() => (cur = null));
  });
  const chains = $derived((cur?.chains ?? []).filter((c) => !hideDone || !c.done));
</script>

{#if !app.view?.analyzed}
  <div class="empty">Контексты перезаписи появятся после анализа.</div>
{:else if !cur}
  <div class="list">
    {#each list as c (c.id)}
      <button class="ctx" onclick={() => (app.focusContext = c.id)}>
        <div class="row"><b>#{c.id}</b><span class="muted small">{c.files.toLocaleString('ru-RU')} конфликтных файлов</span></div>
        <div class="small muted ellipsis">{c.mods.join(' · ')}{c.mods.length >= 12 ? ' …' : ''}</div>
      </button>
    {/each}
    {#if !list.length}<div class="empty">Конфликтов нет.</div>{/if}
  </div>
{:else}
  <div class="wrap">
    <div class="row tools">
      <button class="btn sm" onclick={() => (app.focusContext = -1)}>‹ Контексты</button>
      <b>#{cur.id}</b>
      <span class="muted small">{cur.mods.length} модов · {cur.chains.length} цепочек · решено {cur.done}</span>
      <span class="grow"></span>
      <label class="small row" style="gap:4px"><input type="checkbox" bind:checked={hideDone} /> скрыть решённые</label>
    </div>
    <div class="mods">
      {#each cur.mods as m}
        <button class="m" style="background:{colorToBg(app.byName.get(m)?.color, true)}" onclick={() => app.select(m)}>{m}</button>
      {/each}
    </div>
    <div class="chains">
      {#each chains.slice(0, 400) as c (c.id)}
        <button class="chain" class:active={c.id === chainId} class:done={c.done} onclick={() => (chainId = c.id)}>
          <span class="small muted">{c.files}</span>
          <span class="ellipsis">{c.mods.join(' › ')}</span>
        </button>
      {/each}
    </div>
    {#if chainId >= 0}
      <div class="files"><PathTable reloadKey={String(chainId)} load={(f, o) => api.chainFiles(chainId, f, o)} /></div>
    {/if}
  </div>
{/if}

<style>
  .list { padding: 6px; display: flex; flex-direction: column; gap: 5px; overflow: auto; height: 100%; }
  .ctx { text-align: left; border: 1px solid var(--line); border-radius: 10px; padding: 8px 10px; background: rgba(255,255,255,.025); cursor: pointer; }
  .ctx:hover { background: rgba(255,255,255,.05); }
  .wrap { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .tools { padding: 8px; border-bottom: 1px solid var(--line); }
  .mods { display: flex; flex-wrap: wrap; gap: 3px; padding: 6px 8px; max-height: 92px; overflow: auto; border-bottom: 1px solid var(--line); }
  .m { font-size: 11px; padding: 1px 7px; border-radius: 6px; border: 1px solid rgba(255,255,255,.12); cursor: pointer; }
  .chains { flex: 0 1 35%; overflow: auto; padding: 4px; border-bottom: 1px solid var(--line); }
  .chain { width: 100%; display: flex; gap: 8px; text-align: left; padding: 4px 8px; border: 0; border-radius: 7px; background: transparent; cursor: pointer; font-size: 12px; }
  .chain:hover { background: rgba(255,255,255,.05); }
  .chain.active { background: rgba(101,168,255,.12); }
  .chain.done { opacity: .5; }
  .files { flex: 1 1 auto; min-height: 0; }
</style>
