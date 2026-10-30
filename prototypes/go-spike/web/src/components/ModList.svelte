<script lang="ts">
  import type { Row } from '../lib/api';
  import { app, colorToBg } from '../lib/store.svelte';
  import { modMenu, sepMenu, selectedOr } from '../lib/actions';
  import { drag } from '../lib/drag.svelte';
  import { menu } from './ContextMenu.svelte';
  import VirtualList from './VirtualList.svelte';
  import StatStrip from './StatStrip.svelte';

  const ROW_H = 38;
  let dropAt = $state<{ i: number; after: boolean } | null>(null);

  const header = $derived.by(() => {
    if (app.viewMode === 'explorer') return 'весь список';
    if (app.currentSep === -2) return 'все';
    if (app.currentSep === -1) return 'без разделителя';
    return app.rows[app.currentSep]?.display ?? '';
  });
  const enabledCount = $derived(app.visibleRows.filter((r) => !r.sep && r.en).length);
  const focusIndex = $derived(app.visibleRows.findIndex((r) => r.name === app.focus));

  function click(e: MouseEvent, r: Row) {
    if (e.altKey && !r.sep) { app.toggle([r.name]); return; }
    if (r.sep) {
      if (app.collapsed.has(r.name)) app.collapsed.delete(r.name); else app.collapsed.add(r.name);
      return;
    }
    app.select(r.name, e);
  }

  function dragStart(e: DragEvent, r: Row) {
    if (r.sep) { drag.start(e, 'sep', [r.name]); return; }
    if (!app.selection.has(r.name)) app.select(r.name);
    // keep display order of the dragged set
    const set = new Set(selectedOr(r.name));
    drag.start(e, 'mod', app.rows.filter((x) => set.has(x.name)).map((x) => x.name));
  }

  function over(e: DragEvent, r: Row) {
    e.preventDefault();
    const box = (e.currentTarget as HTMLElement).getBoundingClientRect();
    dropAt = { i: r.i, after: e.clientY > box.top + box.height / 2 };
  }

  function drop(e: DragEvent, r: Row) {
    e.preventDefault();
    const at = dropAt;
    dropAt = null;
    if (!drag.names.length || !at) return;
    let before = r.name;
    if (at.after) {
      // after a collapsed/explorer separator header means "into it, at the top"
      before = app.rows[r.i + 1]?.name ?? '';
    }
    app.move(drag.names, before);
    drag.end();
  }

  function keydown(e: KeyboardEvent) {
    const list = app.visibleRows.filter((r) => !r.sep);
    const i = list.findIndex((r) => r.name === app.focus);
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const n = list[Math.max(0, Math.min(list.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))];
      if (n) app.select(n.name, e);
    } else if (e.key === ' ' && app.selection.size) {
      e.preventDefault();
      app.toggle([...app.selection]);
    } else if (e.key === 'a' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      list.forEach((r) => app.selection.add(r.name));
    }
  }
  const tagsOf = (r: Row) => [...(r.u?.tags ?? [])];
</script>

<div class="panel">
  <div class="panel-header">
    Моды <span class="muted small ellipsis">{header} · {enabledCount}/{app.visibleRows.filter((r) => !r.sep).length}</span>
    <span class="grow"></span>
    {#if app.selection.size > 1}<span class="chip">выбрано {app.selection.size}</span>{/if}
    {#if app.tagFilter}<button class="chip tag" onclick={() => (app.tagFilter = '')} title="Сбросить фильтр тега"># {app.tagFilter} ×</button>{/if}
  </div>
  <div class="search">
    <input class="field" placeholder="Фильтр модов…" bind:value={app.modSearch} />
  </div>
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="panel-content body" tabindex="0" onkeydown={keydown} role="listbox">
    {#if !app.visibleRows.length}
      <div class="empty">Нет модов для этого фильтра.</div>
    {:else}
      <VirtualList items={app.visibleRows} itemHeight={ROW_H} scrollToIndex={focusIndex}>
        {#snippet row(r: Row)}
          {#if r.sep}
            <div
              class="sephdr"
              class:drop-before={dropAt?.i === r.i && !dropAt.after}
              class:drop-after={dropAt?.i === r.i && dropAt.after}
              style="background:{colorToBg(r.color, true)}"
              role="option"
              aria-selected="false"
              tabindex="-1"
              draggable="true"
              ondragstart={(e) => dragStart(e, r)}
              ondragend={() => { drag.end(); dropAt = null; }}
              ondragover={(e) => over(e, r)}
              ondrop={(e) => drop(e, r)}
              onclick={(e) => click(e, r)}
              onkeydown={() => {}}
              oncontextmenu={(e) => menu.show(e, sepMenu(r))}
            >
              <span class="caret">{app.collapsed.has(r.name) ? '▸' : '▾'}</span>
              <span class="grow ellipsis">{r.display}</span>
            </div>
          {:else}
            <div
              class="card"
              class:disabled={!r.en}
              class:selected={app.selection.has(r.name)}
              class:focus={app.focus === r.name}
              class:missing={r.missing}
              class:drop-before={dropAt?.i === r.i && !dropAt.after}
              class:drop-after={dropAt?.i === r.i && dropAt.after}
              style="background:{colorToBg(r.color, r.en)}"
              role="option"
              aria-selected={app.selection.has(r.name)}
              tabindex="-1"
              draggable={!r.unm}
              ondragstart={(e) => dragStart(e, r)}
              ondragend={() => { drag.end(); dropAt = null; }}
              ondragover={(e) => over(e, r)}
              ondrop={(e) => drop(e, r)}
              onclick={(e) => click(e, r)}
              onkeydown={() => {}}
              oncontextmenu={(e) => { if (!app.selection.has(r.name)) app.select(r.name); menu.show(e, modMenu(r)); }}
              title={r.missing ? 'Папка мода не найдена ни в одной из папок mods' : ''}
            >
              <button
                class="ind"
                class:on={r.en}
                class:plg={r.plg}
                title="Alt+клик / клик по точке: включить/выключить"
                onclick={(e) => { e.stopPropagation(); if (!r.unm) app.toggle([r.name]); }}
                aria-label="toggle"
              ></button>
              {#if r.u?.emoji}<span>{r.u.emoji}</span>{/if}
              <span class="name ellipsis">{r.display}</span>
              {#if r.ver}<span class="ver small">{r.ver}</span>{/if}
              <span class="grow"></span>
              {#each tagsOf(r).slice(0, 3) as t}<span class="chip tag">{t}</span>{/each}
              <StatStrip st={r.st} compact />
              {#if r.prio}<span class="prio small">{r.prio}</span>{/if}
            </div>
          {/if}
        {/snippet}
      </VirtualList>
    {/if}
  </div>
  {#if app.view?.overwrite?.st && (app.view.overwrite.st.files ?? 0) > 0}
    <div class="ow">
      <div class="card overwrite" class:focus={app.focus === 'Overwrite'} role="button" tabindex="0"
        onclick={() => app.select('Overwrite')} onkeydown={() => {}}>
        <span class="ind on"></span><span class="name">Overwrite</span><span class="grow"></span>
        <span class="muted small">{app.view.overwrite.st.files} файлов</span>
        <StatStrip st={app.view.overwrite.st} compact />
      </div>
    </div>
  {/if}
</div>

<style>
  .search { padding: 8px; border-bottom: 1px solid var(--line); }
  .search input { width: 100%; }
  .body { overflow: hidden; outline: none; padding: 0; }
  .card, .sephdr {
    height: 34px; margin: 2px 8px; display: flex; align-items: center; gap: 7px; padding: 0 10px; border-radius: 10px;
    border: 1px solid rgba(255,255,255,.13); cursor: pointer; user-select: none; position: relative;
    box-shadow: 0 2px 8px rgba(0,0,0,.18);
  }
  .card:hover { filter: brightness(1.12); }
  .card.disabled { filter: saturate(.55) brightness(.72); }
  .card.disabled:hover { filter: saturate(.6) brightness(.85); }
  .card.selected { outline: 2px solid var(--accent2); outline-offset: -1px; }
  .card.focus { box-shadow: 0 0 0 1px var(--accent2) inset, 0 2px 10px rgba(101,168,255,.25); }
  .card.missing .name { text-decoration: line-through; opacity: .7; }
  .sephdr { font-weight: 750; margin-top: 6px; height: 30px; letter-spacing: .2px; }
  .caret { width: 12px; opacity: .8; }
  .drop-before::before, .drop-after::after {
    content: ''; position: absolute; left: 6px; right: 6px; height: 3px; border-radius: 3px; background: #fff;
    box-shadow: 0 0 8px var(--accent);
  }
  .drop-before::before { top: -4px; }
  .drop-after::after { bottom: -4px; }
  .ind { width: 12px; height: 12px; border-radius: 99px; border: 0; padding: 0; background: #475569; flex: 0 0 auto; cursor: pointer; }
  .ind.on { background: #22c55e; }
  .ind.on.plg { background: #38bdf8; }
  .name { min-width: 0; font-weight: 550; }
  .ver { opacity: .65; }
  .prio { min-width: 30px; text-align: right; opacity: .7; font-variant-numeric: tabular-nums; }
  .ow { border-top: 1px solid var(--line); padding: 4px 0; }
  .overwrite { background: linear-gradient(180deg, rgba(100,116,139,.45), rgba(100,116,139,.25)); }
</style>
