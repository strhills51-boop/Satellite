<script lang="ts">
  import { app, colorToBg } from '../lib/store.svelte';
  import { afterSeparatorBlock, sepMenu } from '../lib/actions';
  import { menu } from './ContextMenu.svelte';
  import { drag } from '../lib/drag.svelte';

  type SepInfo = { i: number; name: string; display: string; color?: string; on: number; off: number; w: number; l: number };

  const info = $derived.by(() => {
    const m = new Map<number, SepInfo>();
    let loose: SepInfo = { i: -1, name: '', display: 'Без разделителя', on: 0, off: 0, w: 0, l: 0 };
    for (const r of app.rows) {
      if (r.sep) { m.set(r.i, { i: r.i, name: r.name, display: r.display, color: r.color, on: 0, off: 0, w: 0, l: 0 }); continue; }
      if (r.unm) continue;
      const s = r.sepI >= 0 ? m.get(r.sepI)! : loose;
      if (r.en) s.on++; else s.off++;
      if (r.st) { s.w += r.st.win; s.l += r.st.lose; }
    }
    const list = [...m.values()];
    return { list, loose };
  });

  const q = $derived(app.sepSearch.trim().toLowerCase());
  const shown = $derived(q ? info.list.filter((s) => s.display.toLowerCase().includes(q)) : info.list);
  const modHits = $derived(q.length >= 2 ? app.rows.filter((r) => !r.sep && r.name.toLowerCase().includes(q)).slice(0, 30) : []);

  let dropOn = $state(-99);

  function onDrop(e: DragEvent, s: SepInfo) {
    e.preventDefault();
    dropOn = -99;
    const names = drag.names;
    if (!names.length) return;
    if (drag.kind === 'sep') {
      app.move(names, s.name); // separators reorder: before target separator
    } else {
      app.move(names, afterSeparatorBlock(s.i));
    }
    drag.end();
  }
  const k = (n: number) => (n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
</script>

<div class="panel">
  <div class="panel-header">Разделители <span class="muted small">{info.list.length}</span></div>
  <div class="search">
    <input class="field" placeholder="Поиск разделителей и модов..." bind:value={app.sepSearch} />
    {#if app.sepSearch}<button class="btn ghost icon sm x" onclick={() => (app.sepSearch = '')} aria-label="Очистить">×</button>{/if}
  </div>
  {#if modHits.length}
    <div class="hits">
      {#each modHits as r}
        <button class="hit" onclick={() => { app.currentSep = r.sepI; app.viewMode = 'classic'; app.select(r.name); }}>
          <span class="ellipsis grow">{r.name}</span>
          <span class="muted small ellipsis">{r.sepI >= 0 ? app.rows[r.sepI].display : ''}</span>
        </button>
      {/each}
    </div>
  {/if}
  <div class="panel-content list">
    <button class="sep all" class:active={app.currentSep === -2} onclick={() => (app.currentSep = -2)}>
      <span class="grow">Все моды</span><span class="muted small">{app.rows.filter((r) => !r.sep).length}</span>
    </button>
    {#if info.loose.on + info.loose.off > 0}
      <button class="sep" class:active={app.currentSep === -1} onclick={() => (app.currentSep = -1)}>
        <span class="grow muted">Без разделителя</span><span class="muted small">{info.loose.on + info.loose.off}</span>
      </button>
    {/if}
    {#each shown as s (s.name)}
      <div
        class="sep"
        class:active={app.currentSep === s.i}
        class:drop={dropOn === s.i}
        style="background:{colorToBg(s.color, true)}"
        role="button"
        tabindex="0"
        draggable="true"
        ondragstart={(e) => drag.start(e, 'sep', [s.name])}
        ondragend={() => { drag.end(); dropOn = -99; }}
        ondragover={(e) => { e.preventDefault(); dropOn = s.i; }}
        ondragleave={() => dropOn === s.i && (dropOn = -99)}
        ondrop={(e) => onDrop(e, s)}
        onclick={() => { app.currentSep = s.i; }}
        onkeydown={(e) => e.key === 'Enter' && (app.currentSep = s.i)}
        oncontextmenu={(e) => menu.show(e, sepMenu(app.rows[s.i]))}
      >
        <span class="grow ellipsis name">{s.display}</span>
        {#if s.w || s.l}<span class="ow small" title="Выигрывает / проигрывает файлов внутри разделителя"><b class="w">{k(s.w)}</b>/<b class="l">{k(s.l)}</b></span>{/if}
        <span class="cnt small" title="Включено / выключено"><span>{s.on}</span><i class="dot on"></i>{#if s.off}<span>{s.off}</span><i class="dot off"></i>{/if}</span>
      </div>
    {/each}
  </div>
</div>

<style>
  .search { position: relative; padding: 8px; border-bottom: 1px solid var(--line); }
  .search input { width: 100%; }
  .search .x { position: absolute; right: 12px; top: 11px; }
  .hits { max-height: 40%; overflow: auto; border-bottom: 1px solid var(--line); padding: 4px; }
  .hit { width: 100%; display: flex; gap: 8px; align-items: center; padding: 5px 8px; border: 0; background: transparent; border-radius: 8px; cursor: pointer; text-align: left; }
  .hit:hover { background: #2a3647; }
  .list { padding: 6px; display: flex; flex-direction: column; gap: 4px; }
  .sep {
    display: flex; align-items: center; gap: 6px; min-height: 34px; padding: 6px 10px; border-radius: 10px;
    border: 1px solid rgba(255,255,255,.12); cursor: pointer; user-select: none; text-align: left; background: #243041;
  }
  .sep.all { background: #1f2836; }
  .sep:hover { filter: brightness(1.12); }
  .sep.active { outline: 2px solid var(--accent2); outline-offset: -1px; }
  .sep.drop { outline: 2px dashed #fff; }
  .name { font-weight: 650; }
  .cnt { display: inline-flex; align-items: center; gap: 3px; opacity: .9; }
  .dot { width: 8px; height: 8px; border-radius: 99px; display: inline-block; margin-right: 3px; }
  .dot.on { background: var(--ok); }
  .dot.off { background: #64748b; }
  .ow { opacity: .85; }
  .ow .w { color: #86efac; font-weight: 600; }
  .ow .l { color: #fca5a5; font-weight: 600; }
</style>
