<script lang="ts">
  import { api, type ScanOptions } from '../lib/api';
  import { app } from '../lib/store.svelte';

  let opt = $state<ScanOptions>({ force: false, dds: true, archives: true, plugins: true, records: false, onlyActive: false });
  let logEl = $state<HTMLDivElement>();
  const run = $derived(app.run);
  const started = $derived(!!run && (run.active || run.stages.some((s) => s.status !== 'pending')));

  $effect(() => {
    void run?.log.length;
    if (logEl) logEl.scrollTop = logEl.scrollHeight;
  });

  async function start() {
    const r = await app.act(() => api.scan(opt));
    if (r) app.run = await api.run();
  }
  const icon: Record<string, string> = { pending: '○', active: '◐', done: '●', error: '✕', skipped: '–' };
  const elapsed = $derived.by(() => {
    if (!run?.startedAt) return '';
    const end = run.active ? Date.now() : new Date(run.endedAt).getTime();
    const s = (end - new Date(run.startedAt).getTime()) / 1000;
    return s > 0 && s < 1e6 ? `${s.toFixed(1)}s` : '';
  });
</script>

<div class="backdrop">
  <div class="modal run">
    <div class="row"><h3 class="grow">Пробег анализа</h3><span class="muted small">{elapsed}</span></div>
    <div class="muted small">Папки модов: {app.view?.instance?.modsDir}{#each app.view?.instance?.extraModDirs ?? [] as d} · {d}{/each} · overwrite: {app.view?.instance?.overwriteDir}</div>

    {#if !run?.active}
      <div class="card opts">
        <label><input type="checkbox" bind:checked={opt.dds} /> 🖼️ VRAM DDS — читать заголовки текстур (точный VRAM)</label>
        <label><input type="checkbox" bind:checked={opt.archives} /> 📦 BSA/BA2 — содержимое архивов как виртуальные пути</label>
        <label><input type="checkbox" bind:checked={opt.plugins} /> 🧩 Плагины — TES4 header, мастера, тип</label>
        <label><input type="checkbox" bind:checked={opt.records} disabled={!opt.plugins} /> 🔎 Record scan — сигнатуры записей, REFR/ACHR (читает плагины целиком)</label>
        <label><input type="checkbox" bind:checked={opt.onlyActive} /> Только включённые моды</label>
        <label><input type="checkbox" bind:checked={opt.force} /> Полный пересчёт (игнорировать кеш сигнатур)</label>
      </div>
    {/if}

    {#if started && run}
      <div class="stages">
        {#each run.stages as s (s.id)}
          <div class="stage {s.status}">
            <div class="row"><span class="ic">{icon[s.status] ?? '○'}</span><b class="grow">{s.title}</b><span class="muted small">{s.status === 'active' ? Math.round(s.percent) + '%' : ''}</span></div>
            {#if s.text}<div class="small muted txt">{s.text}</div>{/if}
            {#if s.status === 'active'}<div class="bar"><i style="width:{s.percent}%"></i></div>{/if}
          </div>
        {/each}
      </div>
      <div class="log mono" bind:this={logEl}>
        {#each run.log as l}<div>{l}</div>{/each}
      </div>
      {#if run.error}<div class="err">{run.error}</div>{/if}
    {/if}

    <div class="row" style="justify-content:flex-end; margin-top:12px">
      {#if !run?.active}
        <button class="btn" onclick={() => (app.modal = '')}>{started ? 'Принять' : 'Закрыть'}</button>
        <button class="btn primary" onclick={start}>{started ? 'Запустить снова' : 'Запустить пробег'}</button>
      {:else}
        <span class="muted small">Можно закрыть окно — пробег продолжится в фоне.</span>
        <button class="btn" onclick={() => (app.modal = '')}>Свернуть</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .run { width: min(820px, calc(100vw - 22px)); }
  .opts { margin-top: 12px; display: flex; flex-direction: column; gap: 6px; }
  .opts label { display: flex; gap: 8px; align-items: center; cursor: pointer; }
  .stages { margin-top: 12px; display: flex; flex-direction: column; gap: 6px; }
  .stage { border: 1px solid var(--line); border-radius: 10px; padding: 7px 10px; background: rgba(255,255,255,.02); }
  .stage.active { border-color: var(--accent); }
  .stage.done .ic { color: var(--ok); }
  .stage.error { border-color: var(--danger); }
  .stage.pending { opacity: .55; }
  .ic { width: 16px; }
  .txt { margin: 2px 0 0 24px; }
  .bar { margin: 6px 0 0 24px; }
  .log { margin-top: 10px; height: 150px; overflow: auto; background: #0d1219; border: 1px solid var(--line); border-radius: 10px; padding: 8px; color: #a9b7ca; }
  .err { color: #fca5a5; margin-top: 8px; }
</style>
