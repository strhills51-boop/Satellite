<script lang="ts">
  import { fmtBytes, type Page, type PathView } from '../lib/api';
  import { errText, app, colorToBg } from '../lib/store.svelte';

  let {
    load,
    reloadKey = '',
    placeholder = 'Фильтр путей…',
  }: { load: (filter: string, offset: number) => Promise<Page<PathView>>; reloadKey?: string; placeholder?: string } = $props();

  let filter = $state('');
  let page = $state<Page<PathView> | null>(null);
  let offset = $state(0);
  let loading = $state(false);
  let err = $state('');
  const LIMIT = 200;

  $effect(() => {
    void reloadKey;
    void app.gen;
    const f = filter;
    const o = offset;
    loading = true;
    const t = setTimeout(() => {
      load(f, o)
        .then((p) => { page = p; err = ''; })
        .catch((e) => (err = errText(e)))
        .finally(() => (loading = false));
    }, 120);
    return () => clearTimeout(t);
  });
  const colorOf = (mod: string) => app.byName.get(mod)?.color;
</script>

<div class="pt">
  <div class="row tools">
    <input class="field grow" {placeholder} bind:value={filter} oninput={() => (offset = 0)} />
    {#if page}<span class="muted small">{page.total.toLocaleString('ru-RU')}</span>{/if}
    <button class="btn sm" disabled={offset === 0} onclick={() => (offset = Math.max(0, offset - LIMIT))}>‹</button>
    <button class="btn sm" disabled={!page || offset + LIMIT >= page.total} onclick={() => (offset += LIMIT)}>›</button>
  </div>
  {#if err}<div class="empty">{err}</div>{/if}
  <div class="scroll" class:dim={loading}>
    {#each page?.items ?? [] as p (p.path)}
      <div class="path">
        <div class="mono pathname" title={p.path}>{p.path}</div>
        <div class="stack">
          {#each p.stack as s, i}
            {#if i > 0}<span class="arrow">›</span>{/if}
            <button
              class="own"
              class:win={s.winner}
              class:inactive={!s.active}
              style="background:{colorToBg(colorOf(s.mod), s.active)}"
              title="{s.mod}{s.archive ? ' · из ' + s.archive : ''} · {fmtBytes(s.size)}{s.vramKiB ? ' · VRAM ' + fmtBytes(s.vramKiB * 1024) : ''}{s.active ? '' : ' · выключен'}"
              onclick={() => app.select(s.mod)}
            >
              {#if s.winner}<b>W</b>{/if}
              <span class="ellipsis">{s.mod}</span>
              {#if s.archive}<span class="arc">📦</span>{/if}
            </button>
          {/each}
        </div>
      </div>
    {/each}
    {#if page && !page.items.length && !loading}<div class="empty">Ничего не найдено.</div>{/if}
  </div>
</div>

<style>
  .pt { display: flex; flex-direction: column; min-height: 0; height: 100%; }
  .tools { padding: 8px; border-bottom: 1px solid var(--line); }
  .scroll { overflow: auto; flex: 1 1 auto; min-height: 0; }
  .scroll.dim { opacity: .6; }
  .path { padding: 6px 10px; border-bottom: 1px solid rgba(52,65,85,.55); }
  .pathname { color: #cbd5e1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .stack { display: flex; flex-wrap: wrap; gap: 3px; margin-top: 4px; align-items: center; }
  .own {
    display: inline-flex; gap: 4px; align-items: center; max-width: 220px; height: 22px; padding: 0 8px; border-radius: 7px;
    border: 1px solid rgba(255,255,255,.14); font-size: 11.5px; cursor: pointer;
  }
  .own.win { border-color: rgba(134,239,172,.8); box-shadow: 0 0 0 1px rgba(34,197,94,.35); }
  .own.win b { color: #86efac; }
  .own.inactive { opacity: .5; }
  .arrow { color: var(--muted); font-size: 12px; }
  .arc { font-size: 10px; }
</style>
