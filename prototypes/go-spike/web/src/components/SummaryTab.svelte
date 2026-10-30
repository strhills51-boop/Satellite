<script lang="ts">
  import { api, fmtBytes, fmtN, type ModDetails } from '../lib/api';
  import { errText, app } from '../lib/store.svelte';
  import StatStrip from './StatStrip.svelte';

  let d = $state<ModDetails | null>(null);
  let err = $state('');
  let tagInput = $state('');
  let note = $state('');

  $effect(() => {
    const name = app.focus;
    void app.gen;
    void app.view;
    if (!name) { d = null; return; }
    api.mod(name).then((x) => { d = x; note = x.user?.note ?? ''; err = ''; }).catch((e) => (err = errText(e)));
  });

  async function addTag() {
    const t = tagInput.trim();
    if (!t || !d) return;
    const names = app.selection.has(d.name) ? [...app.selection] : [d.name];
    await app.act(() => api.user({ kind: 'mod', names, addTags: [t] }));
    tagInput = '';
  }
  const rmTag = (t: string) => d && app.act(() => api.user({ kind: 'mod', names: [d!.name], removeTags: [t] }));
  const saveNote = () => d && app.act(() => api.user({ kind: 'mod', names: [d!.name], patch: { note } }), 'Заметка сохранена');
  const topExt = (m: Record<string, number> | undefined) =>
    Object.entries(m ?? {}).sort((a, b) => b[1] - a[1]).slice(0, 8);
  const nexus = (id: string) => `https://www.nexusmods.com/skyrimspecialedition/mods/${id}`;
  const s = $derived(app.summary);
</script>

{#if !app.focus}
  {#if s}
    <div class="pad col">
      <div class="card">
        <div class="h">Сборка</div>
        <div class="kv">
          <span>Моды</span><span>{fmtN(s.Mods)} · вкл {fmtN(s.Enabled)} · выкл {fmtN(s.Disabled)}</span>
          <span>Разделители</span><span>{fmtN(s.Separators)}</span>
          <span>Размер</span><span>{fmtBytes(s.Bytes)}</span>
          <span>Файлы</span><span>{fmtN(s.Files)} · путей VFS {fmtN(s.Paths)}</span>
          <span>Конфликты</span><span>{fmtN(s.ConflictPaths)} путей · {fmtN(s.Chains)} цепочек · {fmtN(s.Contexts)} контекстов</span>
          <span>Полностью перекрыты</span><span>{fmtN(s.FullyOverridden)} модов</span>
        </div>
      </div>
      <div class="card">
        <div class="h">Ресурсы</div>
        <div class="kv">
          <span>VRAM DDS (все)</span><span>{fmtBytes(s.VRAMKnownBytes)}</span>
          <span>VRAM DDS (победители)</span><span title="Сумма VRAM текстур, которые реально попадут в игру (без перекрытых)">{fmtBytes(s.VRAMWinBytes)}</span>
          <span>Плагины</span><span>{s.PluginsActive} активны из {s.Plugins} · ESM {s.ESM} · ESP {s.ESP} · ESL {s.ESL}</span>
          <span>Полные слоты</span><span class:bad={s.FullSlots > 254}>{s.FullSlots} / 254</span>
          <span>Содержимое</span><span>DDS {fmtN(s.DDS)} · NIF {fmtN(s.NIF)} · PEX {fmtN(s.PEX)} · HKX {fmtN(s.HKX)} · DLL {fmtN(s.DLL)} · архивов {fmtN(s.Archives)}</span>
        </div>
      </div>
      <div class="card small muted">
        {#if s.LastScan && !s.LastScan.startsWith('0001')}Последний пробег: {new Date(s.LastScan).toLocaleString()}{:else}Анализ ещё не запускался — «Анализ mods» в шапке.{/if}
        <br />Workspace: <span class="mono">{s.Workspace}</span>
      </div>
      <div class="muted small">Выдели мод, чтобы увидеть его сводку.</div>
    </div>
  {/if}
{:else if err}
  <div class="empty">{err}</div>
{:else if d}
  <div class="pad col">
    <div class="card">
      <div class="row"><b class="grow ellipsis title">{d.name}</b><StatStrip st={d.stat} /></div>
      <div class="kv" style="margin-top:8px">
        {#if d.meta.version}<span>Версия</span><span>{d.meta.version}{#if d.meta.newest && d.meta.newest !== d.meta.version} · новее: {d.meta.newest}{/if}</span>{/if}
        {#if d.meta.modId}<span>Nexus</span><span><a href={nexus(d.meta.modId)} target="_blank" rel="noreferrer">{d.meta.modId}</a></span>{/if}
        {#if d.meta.install}<span>Архив</span><span class="ellipsis">{d.meta.install}</span>{/if}
        <span>Папка</span><span class="mono ellipsis" title={d.path}>{d.path}</span>
        {#if d.missing}<span>Статус</span><span class="bad">папка не найдена</span>{/if}
        {#if d.error}<span>Ошибка</span><span class="bad">{d.error}</span>{/if}
      </div>
      <div class="row" style="margin-top:8px">
        <button class="btn sm" onclick={() => app.act(() => api.reveal(d!.name))}>📂 Открыть</button>
        <button class="btn sm" onclick={() => (app.rightTab = 'conflicts')}>⚔ Конфликты</button>
        <button class="btn sm" onclick={() => (app.rightTab = 'tree')}>🌲 Дерево</button>
      </div>
    </div>

    {#if d.stat}
      <div class="card">
        <div class="h">Перезапись</div>
        <div class="kv">
          <span>Файлов у мода</span><span>{fmtN(d.stat.files)} · уникальных {fmtN(d.stat.unique)}</span>
          <span>Выигрывает</span><span class="w">{fmtN(d.stat.win)}</span>
          <span>Проигрывает</span><span class="l">{fmtN(d.stat.lose)}{#if d.stat.mid} · в середине стека {fmtN(d.stat.mid)}{/if}</span>
          <span>Цепочек</span><span>{fmtN(d.stat.chains)}{#if d.stat.context >= 0} · контекст #{d.stat.context}{/if}</span>
          <span>VRAM в игре</span><span>{fmtBytes(d.stat.vramWinKiB * 1024)}</span>
        </div>
        {#if d.stat.full}<div class="xnote">X — от мода в игре ничего не остаётся: все его файлы перекрыты.</div>{/if}
      </div>
    {/if}

    {#if d.metrics}
      <div class="card">
        <div class="h">Метрики</div>
        <div class="kv">
          <span>Размер</span><span>{fmtBytes(d.metrics.bytes)} · файлов {fmtN(d.metrics.files)} · папок {fmtN(d.metrics.dirs)}</span>
          <span>Содержимое</span><span>DDS {d.metrics.dds} · NIF {d.metrics.nif} · PEX {d.metrics.pex} · HKX {d.metrics.hkx} · DLL {d.metrics.dll}</span>
          <span>VRAM DDS</span><span>{fmtBytes(d.metrics.vramBytes)} ({d.metrics.ddsKnown}/{d.metrics.dds + d.metrics.archiveFiles ? d.metrics.dds : 0} точно)</span>
          {#if d.metrics.archives}<span>Архивы</span><span>{d.metrics.archives} · записей {fmtN(d.metrics.archiveFiles)}</span>{/if}
          {#if d.metrics.nestedArchives || d.metrics.sourceFiles}<span>Cargo?</span><span class="warn">вложенных архивов {d.metrics.nestedArchives} · исходников {d.metrics.sourceFiles}</span>{/if}
          <span>Глубина</span><span>макс {d.metrics.maxDepth} · в корне {d.metrics.rootFiles}</span>
          <span>Расширения</span><span class="small">{topExt(d.metrics.ext).map(([k, v]) => `${k || '∅'} ${v}`).join(' · ')}</span>
          <span>Папки</span><span class="small">{topExt(d.metrics.topDirs).map(([k, v]) => `${k} ${v}`).join(' · ')}</span>
        </div>
        <div class="muted small" style="margin-top:6px">{d.cached ? 'из кеша' : 'просканирован'} · {new Date(d.scanned).toLocaleString()}</div>
      </div>
    {/if}

    {#if d.plugins?.length || d.archives?.length}
      <div class="card">
        <div class="h">Плагины и архивы</div>
        {#each d.plugins ?? [] as p}
          <div class="pl">
            <span class="chip">{p.light ? 'ESL' : p.master || p.name.toLowerCase().endsWith('.esm') ? 'ESM' : 'ESP'}</span>
            <b class="ellipsis">{p.name}</b>
            <span class="muted small">записей {fmtN(p.numRecords)}{#if p.records} · REFR {fmtN((p.records['REFR'] ?? 0) + (p.records['ACHR'] ?? 0))}{/if}</span>
          </div>
          {#if p.masters?.length}<div class="muted small mast">мастера: {p.masters.join(', ')}</div>{/if}
          {#if p.error}<div class="bad small">{p.error}</div>{/if}
        {/each}
        {#each d.archives ?? [] as a}
          <div class="pl"><span class="chip">{a.kind || 'архив'}</span><b class="ellipsis">{a.name}</b><span class="muted small">{fmtN(a.entries)} файлов</span></div>
          {#if a.error}<div class="bad small">{a.error}</div>{/if}
        {/each}
      </div>
    {/if}

    <div class="card">
      <div class="h">Теги</div>
      <div class="tags">
        {#each d.user?.tags ?? [] as t}<button class="chip tag" title="Снять тег" onclick={() => rmTag(t)}>{t} ×</button>{/each}
        {#each d.user?.auto ?? [] as t}<button class="chip auto" title="Фильтр по тегу" onclick={() => (app.tagFilter = t)}>{t}</button>{/each}
      </div>
      <div class="row" style="margin-top:6px">
        <input class="field grow" placeholder="Добавить тег…" bind:value={tagInput} onkeydown={(e) => e.key === 'Enter' && addTag()} />
        <button class="btn sm" onclick={addTag}>+</button>
      </div>
    </div>
    <div class="card">
      <div class="h">Заметка</div>
      <textarea class="field" rows="3" style="width:100%" bind:value={note} placeholder="Заметка к моду…"></textarea>
      <div class="row" style="justify-content:flex-end; margin-top:6px"><button class="btn sm" onclick={saveNote}>Сохранить</button></div>
    </div>
  </div>
{/if}

<style>
  .pad { padding: 10px; }
  .h { font-weight: 700; margin-bottom: 6px; }
  .title { font-size: 15px; }
  .w { color: #86efac; }
  .l { color: #fca5a5; }
  .bad { color: #fca5a5; }
  .warn { color: #fcd34d; }
  .xnote { margin-top: 8px; padding: 6px 8px; border-radius: 8px; background: rgba(168,85,247,.14); color: #e9d5ff; font-size: 12.5px; }
  .pl { display: flex; align-items: center; gap: 8px; margin-top: 4px; }
  .mast { margin-left: 4px; }
  .tags { display: flex; flex-wrap: wrap; gap: 4px; }
  .tags button { cursor: pointer; }
  a { color: var(--accent2); }
</style>
