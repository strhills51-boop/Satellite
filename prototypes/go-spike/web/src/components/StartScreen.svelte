<script lang="ts">
  import { api, type FsList } from '../lib/api';
  import { errText, app } from '../lib/store.svelte';

  let path = $state(app.status?.config.instance ?? '');
  let extra = $state((app.status?.config.extra ?? []).join('\n'));
  let profile = $state(app.view?.instance?.profile ?? '');
  let browse = $state<FsList | null>(null);
  let browseFor = $state<'inst' | 'extra'>('inst');
  let err = $state('');
  let busy = $state(false);
  const recent = $derived(app.status?.config.recent ?? []);
  const profiles = $derived(app.view?.instance?.profiles ?? []);
  const hasProject = $derived(!!app.view?.instance);

  async function open() {
    busy = true;
    err = '';
    try {
      const v = await api.open(path, extra.split('\n').map((s) => s.trim()).filter(Boolean), profile);
      app.view = v;
      app.status = await api.status();
      app.modal = '';
      app.currentSep = -2;
      app.focus = '';
      app.selection.clear();
      await app.reload();
      if (!v.analyzed) app.modal = 'run';
    } catch (e) {
      err = errText(e).replace(/^Error: /, '');
    } finally {
      busy = false;
    }
  }
  async function nav(p: string) {
    try { browse = await api.fsList(p); } catch (e) { err = errText(e); }
  }
  function pick(p: string) {
    if (browseFor === 'inst') path = p;
    else extra = (extra ? extra + '\n' : '') + p;
    browse = null;
  }
</script>

<div class="backdrop">
  <div class="modal start">
    <div class="hero">
      <div class="mark">M</div>
      <div>
        <div class="t1">ModlistTool.mo2 <span class="muted">| Monolith</span></div>
        <div class="muted small">Новый движок · данные остаются на этом компьютере</div>
      </div>
    </div>

    {#if hasProject}
      <div class="card state">
        <b>С возвращением.</b>
        <div class="small muted">Открыт профиль <b>{app.view?.profile}</b> · {app.view?.rows.length} записей</div>
        <div class="row" style="margin-top:8px">
          <button class="btn primary" onclick={() => (app.modal = '')}>Продолжить работу</button>
        </div>
      </div>
    {/if}

    <div class="col" style="margin-top:12px">
      <label class="small muted" for="inst">Инстанс MO2 — папка с ModOrganizer.ini (или сам файл)</label>
      <div class="row">
        <input id="inst" class="field grow" placeholder="E:\Modding\MO2" bind:value={path} onkeydown={(e) => e.key === 'Enter' && open()} />
        <button class="btn" onclick={() => { browseFor = 'inst'; nav(path); }}>Обзор…</button>
      </div>
      {#if recent.length}
        <div class="row" style="flex-wrap:wrap; gap:4px">
          {#each recent as r}<button class="chip" onclick={() => (path = r)}>{r}</button>{/each}
        </div>
      {/if}
      <label class="small muted" for="extra">Дополнительные папки модов (по одной на строку) — моды ищутся сначала в mod_directory из ModOrganizer.ini, затем здесь</label>
      <div class="row" style="align-items:flex-start">
        <textarea id="extra" class="field grow" rows="2" bind:value={extra} placeholder="F:\MO2\mods2"></textarea>
        <button class="btn" onclick={() => { browseFor = 'extra'; nav(''); }}>Обзор…</button>
      </div>
      {#if profiles.length > 1}
        <label class="small muted" for="prof">Профиль</label>
        <select id="prof" class="field" bind:value={profile}>
          {#each profiles as p}<option value={p}>{p}</option>{/each}
        </select>
      {/if}
      {#if err}<div class="err">{err}</div>{/if}
      <div class="row" style="justify-content:flex-end">
        <button class="btn primary" disabled={!path || busy} onclick={open}>{busy ? 'Открываю…' : 'Открыть профиль'}</button>
      </div>
    </div>

    {#if browse}
      <div class="browser card">
        <div class="row">
          <button class="btn sm" disabled={!browse.path} onclick={() => nav(browse!.parent)}>↑</button>
          <span class="mono ellipsis grow">{browse.path || 'Диски'}</span>
          {#if browse.path}<button class="btn sm primary" onclick={() => pick(browse!.path)}>Выбрать эту папку</button>{/if}
          <button class="btn sm" onclick={() => (browse = null)}>×</button>
        </div>
        <div class="entries">
          {#each browse.entries as e}
            {#if e.dir}
              <button class="ent" class:mo2={e.isMO2} ondblclick={() => (e.isMO2 && browseFor === 'inst' ? pick(e.path) : nav(e.path))} onclick={() => nav(e.path)}>
                📁 {e.name} {#if e.isMO2}<span class="chip w">MO2</span>{/if}
              </button>
            {:else}
              <button class="ent" onclick={() => pick(e.path)}>⚙️ {e.name}</button>
            {/if}
          {/each}
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .start { width: min(720px, calc(100vw - 22px)); }
  .hero { display: flex; gap: 12px; align-items: center; }
  .mark { width: 44px; height: 44px; border-radius: 12px; background: linear-gradient(135deg, #2457a8, #65a8ff); display: grid; place-items: center; font-weight: 900; font-size: 22px; }
  .t1 { font-weight: 800; font-size: 20px; }
  .state { margin-top: 12px; }
  .err { color: #fca5a5; background: rgba(239,68,68,.12); padding: 8px 10px; border-radius: 10px; }
  .browser { margin-top: 10px; }
  .entries { max-height: 300px; overflow: auto; margin-top: 6px; display: flex; flex-direction: column; }
  .ent { text-align: left; border: 0; background: transparent; padding: 4px 8px; border-radius: 7px; cursor: pointer; }
  .ent:hover { background: rgba(255,255,255,.06); }
  .ent.mo2 { font-weight: 700; }
  .chip { cursor: pointer; }
</style>
