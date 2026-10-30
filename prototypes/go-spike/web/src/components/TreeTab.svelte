<script lang="ts">
  import { api, fmtBytes, type TreeEntry } from '../lib/api';
  import { errText, app } from '../lib/store.svelte';

  let dir = $state('');
  let items = $state<TreeEntry[] | null>(null);
  let err = $state('');
  let lastMod = '';
  let filter = $state<'' | 'W' | 'L' | 'U'>('');

  $effect(() => {
    const name = app.focus;
    if (name !== lastMod) { lastMod = name; dir = ''; }
    const d = dir;
    void app.gen;
    if (!name) { items = null; return; }
    api.tree(name, d).then((x) => { items = x; err = ''; }).catch((e) => { err = errText(e); items = null; });
  });
  const crumbs = $derived(dir ? dir.split('/') : []);
  const shown = $derived((items ?? []).filter((e) => !filter || e.dir || e.state === filter));
  const label: Record<string, string> = { W: 'выигрывает', L: 'перекрыт', U: 'уникальный', '-': 'мод выключен' };
</script>

{#if !app.focus}
  <div class="empty">Выдели один мод в любом окне модов, чтобы показать дерево.</div>
{:else if err}
  <div class="empty">{err}</div>
{:else}
  <div class="wrap">
    <div class="row crumbs">
      <button class="btn ghost sm" onclick={() => (dir = '')}>🏠 Data</button>
      {#each crumbs as c, i}
        <span class="muted">/</span>
        <button class="btn ghost sm" onclick={() => (dir = crumbs.slice(0, i + 1).join('/'))}>{c}</button>
      {/each}
      <span class="grow"></span>
      {#each ['', 'W', 'L', 'U'] as f}
        <button class="btn sm" class:primary={filter === f} onclick={() => (filter = f as typeof filter)}>{f || 'все'}</button>
      {/each}
    </div>
    <div class="list">
      {#if dir}
        <button class="ent" onclick={() => (dir = crumbs.slice(0, -1).join('/'))}><span class="ic">↩</span><span class="muted">..</span></button>
      {/if}
      {#each shown as e (e.name + (e.archive ?? ''))}
        {#if e.dir}
          <button class="ent" onclick={() => (dir = dir ? `${dir}/${e.name}` : e.name)}>
            <span class="ic">📁</span><span class="grow ellipsis">{e.name}</span>
            <span class="muted small">{e.files} · {fmtBytes(e.size)}</span>
          </button>
        {:else}
          <div class="ent file" title={label[e.state ?? ''] ?? ''}>
            <span class="ic st {e.state}">{e.state === 'W' ? '▲' : e.state === 'L' ? '▼' : e.state === 'U' ? '•' : '·'}</span>
            <span class="grow ellipsis">{e.name}</span>
            {#if e.archive}<span class="chip small">📦 {e.archive}</span>{/if}
            {#if e.others}<span class="chip {e.state === 'W' ? 'w' : 'l'}">+{e.others}</span>{/if}
            <span class="muted small">{fmtBytes(e.size)}</span>
          </div>
        {/if}
      {/each}
      {#if items && !shown.length}<div class="empty">Папка пуста.</div>{/if}
    </div>
  </div>
{/if}

<style>
  .wrap { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .crumbs { padding: 6px 8px; border-bottom: 1px solid var(--line); flex-wrap: wrap; gap: 2px; }
  .list { overflow: auto; flex: 1; padding: 4px; }
  .ent { width: 100%; display: flex; align-items: center; gap: 8px; padding: 4px 8px; border-radius: 7px; border: 0; background: transparent; text-align: left; cursor: pointer; font-size: 13px; }
  .ent:hover { background: rgba(255,255,255,.05); }
  .ent.file { cursor: default; }
  .ic { width: 18px; text-align: center; flex: 0 0 auto; }
  .st.W { color: #86efac; }
  .st.L { color: #fca5a5; }
  .st.U { color: #93c5fd; }
</style>
