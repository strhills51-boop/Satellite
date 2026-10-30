<script lang="ts">
  import { api, fmtBytes, type ChainView } from '../lib/api';
  import { errText, app, colorToBg } from '../lib/store.svelte';
  import PathTable from './PathTable.svelte';

  let chains = $state<ChainView[] | null>(null);
  let err = $state('');
  let showDone = $state(true);
  let role = $state<'' | 'W' | 'L' | 'M'>('');

  $effect(() => {
    const name = app.focus;
    void app.gen;
    void app.view?.analyzed;
    if (!name) { chains = null; return; }
    api.modChains(name)
      .then((c) => {
        chains = c ?? [];
        err = '';
        if (!chains.some((x) => x.id === app.focusChain)) app.focusChain = chains[0]?.id ?? -1;
      })
      .catch((e) => { err = errText(e); chains = null; });
  });

  const list = $derived((chains ?? []).filter((c) => (showDone || !c.done) && (!role || c.role === role)));
  const sel = $derived(chains?.find((c) => c.id === app.focusChain));
  const toggleDone = (c: ChainView) => app.act(() => api.user({ kind: 'chain', key: c.key, patch: { done: !c.done } }));

  function copyForAI(c: ChainView) {
    api.chainFiles(c.id, '', 0, 2000).then((p) => {
      const lines = [
        `Цепочка перезаписи (победитель первым): ${c.mods.join(' > ')}`,
        `Файлов: ${c.files}`,
        ...p.items.map((x, i) => `${i + 1}. ${x.path} :: winner: ${x.stack.find((s) => s.winner)?.mod ?? '—'} <= overwritten: ${x.stack.filter((s) => !s.winner).map((s) => s.mod).join(', ')}`),
      ];
      navigator.clipboard?.writeText(lines.join('\n'));
      app.notify('Цепочка скопирована для ИИ', 'ok');
    });
  }
</script>

{#if !app.focus}
  <div class="empty">Выдели один мод, чтобы показать его конфликты.</div>
{:else if err}
  <div class="empty">{err}</div>
{:else if chains && !chains.length}
  <div class="empty">Выбранный мод не участвует в цепочках перезаписи.</div>
{:else if chains}
  <div class="wrap">
    <div class="row tools">
      <b class="ellipsis grow">{app.focus}</b>
      {#each ['', 'W', 'L', 'M'] as r}
        <button class="btn sm" class:primary={role === r} onclick={() => (role = r as typeof role)}>{r || 'все'}</button>
      {/each}
      <label class="small row" style="gap:4px"><input type="checkbox" bind:checked={showDone} /> решённые</label>
    </div>
    <div class="chains">
      {#each list as c (c.id)}
        <div class="chain" class:active={c.id === app.focusChain} class:done={c.done} role="button" tabindex="0"
          onclick={() => (app.focusChain = c.id)} onkeydown={(e) => e.key === 'Enter' && (app.focusChain = c.id)}>
          <div class="row">
            <span class="chip {c.role?.toLowerCase()}">{c.role}</span>
            <span class="small muted">{c.files} файл.{c.bytes ? ' · ' + fmtBytes(c.bytes) : ''}</span>
            <span class="grow"></span>
            <button class="btn ghost sm" title="Скопировать цепочку для ИИ" onclick={(e) => { e.stopPropagation(); copyForAI(c); }}>⧉</button>
            <button class="btn ghost sm" title={c.done ? 'Пометить незавершённой' : 'Пометить завершённой'} onclick={(e) => { e.stopPropagation(); toggleDone(c); }}>{c.done ? '✓' : '○'}</button>
          </div>
          <div class="mods">
            {#each c.mods as m, i}
              {#if i > 0}<span class="arrow">›</span>{/if}
              <span class="m" class:me={m === app.focus} style="background:{colorToBg(app.byName.get(m)?.color, true)}">{m}</span>
            {/each}
          </div>
        </div>
      {/each}
    </div>
    {#if sel}
      <div class="files">
        <PathTable reloadKey={String(sel.id)} load={(f, o) => api.chainFiles(sel.id, f, o)} />
      </div>
    {/if}
  </div>
{/if}

<style>
  .wrap { display: flex; flex-direction: column; height: 100%; min-height: 0; }
  .tools { padding: 8px; border-bottom: 1px solid var(--line); }
  .chains { flex: 0 1 45%; overflow: auto; padding: 6px; display: flex; flex-direction: column; gap: 5px; border-bottom: 1px solid var(--line); }
  .files { flex: 1 1 55%; min-height: 0; }
  .chain { border: 1px solid var(--line); border-radius: 10px; padding: 6px 8px; cursor: pointer; background: rgba(255,255,255,.025); }
  .chain:hover { background: rgba(255,255,255,.05); }
  .chain.active { border-color: var(--accent); background: rgba(101,168,255,.08); }
  .chain.done { opacity: .55; }
  .mods { display: flex; flex-wrap: wrap; gap: 3px; margin-top: 4px; align-items: center; }
  .m { font-size: 11.5px; padding: 1px 7px; border-radius: 6px; border: 1px solid rgba(255,255,255,.12); max-width: 240px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .m.me { outline: 1px solid #fff; }
  .arrow { color: var(--muted); font-size: 11px; }
</style>
