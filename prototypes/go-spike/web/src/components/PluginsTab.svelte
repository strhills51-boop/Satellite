<script lang="ts">
  import { api, fmtN, type PluginView } from '../lib/api';
  import { app } from '../lib/store.svelte';
  import VirtualList from './VirtualList.svelte';

  let list = $state<PluginView[]>([]);
  let q = $state('');
  let mode = $state<'all' | 'active' | 'problems' | 'scan-only'>('all');
  let sel = $state('');

  $effect(() => {
    void app.gen;
    void app.view;
    api.plugins().then((x) => (list = x)).catch((e) => app.notify(String(e), 'err'));
  });
  const shown = $derived(
    list.filter((p) => {
      if (q && !p.name.toLowerCase().includes(q.toLowerCase()) && !(p.owner ?? '').toLowerCase().includes(q.toLowerCase())) return false;
      if (mode === 'active') return p.enabled;
      if (mode === 'problems') return !!(p.missingMasters?.length || p.lateMasters?.length || p.error || (!p.found && p.enabled));
      if (mode === 'scan-only') return !p.inLoadorder;
      return true;
    }),
  );
  const cur = $derived(list.find((p) => p.name === sel));
  const dependents = $derived(cur ? list.filter((p) => p.masters?.some((m) => m.toLowerCase() === cur.name.toLowerCase())) : []);
  function copy() {
    navigator.clipboard?.writeText(shown.map((p) => `${p.slot ?? '--'} ${p.name}`).join('\n'));
    app.notify('Список plugins скопирован', 'ok');
  }
</script>

<div class="wrap">
  <div class="row tools">
    <input class="field grow" placeholder="Поиск plugin или мода…" bind:value={q} />
    <select class="field" bind:value={mode}>
      <option value="all">все ({list.length})</option>
      <option value="active">активные</option>
      <option value="problems">проблемы</option>
      <option value="scan-only">вне loadorder</option>
    </select>
    <button class="btn sm" title="Скопировать список plugins" onclick={copy}>⧉</button>
  </div>
  <div class="list">
    <VirtualList items={shown} itemHeight={30}>
      {#snippet row(p: PluginView)}
        <div class="pl" class:off={!p.enabled} class:sel={p.name === sel} role="button" tabindex="0" onclick={() => (sel = p.name)} onkeydown={() => {}}>
          <span class="slot mono">{p.slot ?? '--'}</span>
          <span class="chip k {p.kind}">{p.kind}</span>
          <span class="grow ellipsis">{p.name}</span>
          {#if p.missingMasters?.length}<span class="chip l" title="Нет мастеров: {p.missingMasters.join(', ')}">нет мастера</span>{/if}
          {#if p.lateMasters?.length}<span class="chip m" title="Мастер грузится позже: {p.lateMasters.join(', ')}">порядок</span>{/if}
          {#if !p.found && p.enabled && !p.name.match(/^(skyrim|update|dawnguard|hearthfires|dragonborn)\.esm$/i)}<span class="chip x" title="Не найден в просканированных модах">?</span>{/if}
          {#if !p.inLoadorder}<span class="chip" title="Найден сканом, но его нет в loadorder.txt">scan</span>{/if}
          <span class="muted small ellipsis owner">{p.owner ?? ''}</span>
        </div>
      {/snippet}
    </VirtualList>
  </div>
  {#if cur}
    <div class="det card">
      <div class="row"><b class="grow ellipsis">{cur.name}</b><span class="chip">{cur.kind}</span>{#if cur.slot}<span class="chip">{cur.slot}</span>{/if}</div>
      <div class="kv small" style="margin-top:6px">
        <span>Владелец</span><span>{#if cur.owner}<button class="link" onclick={() => app.select(cur.owner!)}>{cur.owner}</button>{:else}—{/if}{#if (cur.owners?.length ?? 0) > 1} · также: {cur.owners!.slice(0, -1).join(', ')}{/if}</span>
        <span>Записей</span><span>{fmtN(cur.records)}{#if cur.refs} · REFR/ACHR {fmtN(cur.refs)}{/if}</span>
        <span>Мастера</span><span>{cur.masters?.join(', ') || '—'}</span>
        {#if cur.missingMasters?.length}<span>Нет</span><span class="bad">{cur.missingMasters.join(', ')}</span>{/if}
        {#if cur.lateMasters?.length}<span>Позже</span><span class="warn">{cur.lateMasters.join(', ')}</span>{/if}
        <span>Зависят</span><span>{dependents.map((d) => d.name).join(', ') || '—'}</span>
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .tools { padding: 8px; border-bottom: 1px solid var(--line); }
  .list { flex: 1 1 auto; min-height: 0; }
  .pl { height: 28px; margin: 1px 6px; display: flex; align-items: center; gap: 6px; padding: 0 8px; border-radius: 7px; cursor: pointer; font-size: 12.5px; }
  .pl:hover { background: rgba(255,255,255,.05); }
  .pl.sel { background: rgba(101,168,255,.12); }
  .pl.off { opacity: .5; }
  .slot { width: 46px; color: var(--muted); }
  .k { height: 18px; font-size: 10px; }
  .k.ESM { color: #fcd34d; }
  .k.ESL { color: #93c5fd; }
  .owner { max-width: 35%; }
  .det { margin: 8px; }
  .bad { color: #fca5a5; }
  .warn { color: #fcd34d; }
  .link { border: 0; background: none; color: var(--accent2); cursor: pointer; padding: 0; }
</style>
