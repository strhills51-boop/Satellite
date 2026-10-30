import { SvelteSet } from 'svelte/reactivity';
import { api, subscribe, type ModlistView, type Row, type RunState, type Status, type Summary } from './api';

export type RightTab = 'summary' | 'conflicts' | 'tree' | 'plugins' | 'contexts' | 'files';

class AppState {
  status = $state<Status | null>(null);
  view = $state<ModlistView | null>(null);
  summary = $state<Summary | null>(null);
  run = $state<RunState | null>(null);
  viewMode = $state<'classic' | 'explorer'>('classic');
  currentSep = $state<number>(-2); // row index of separator; -2 = all, -1 = without separator
  selection = new SvelteSet<string>();
  anchor = $state<string>('');
  focus = $state<string>(''); // mod shown in the right panel
  rightTab = $state<RightTab>('summary');
  modSearch = $state('');
  sepSearch = $state('');
  tagFilter = $state('');
  collapsed = new SvelteSet<string>();
  modal = $state<'' | 'start' | 'run' | 'history' | 'settings' | 'help'>('');
  toast = $state<{ text: string; kind: 'ok' | 'err' | 'info' } | null>(null);
  busy = $state(false);
  gen = $state(0); // bumps when conflicts change -> panels refetch
  focusChain = $state<number>(-1);
  focusContext = $state<number>(-1);

  rows = $derived(this.view?.rows ?? []);
  seps = $derived(this.rows.filter((r) => r.sep));
  byName = $derived(new Map(this.rows.map((r) => [r.name, r] as const)));

  /** mods visible in the middle panel (classic: current separator only). */
  visibleRows = $derived.by(() => {
    const q = this.modSearch.trim().toLowerCase();
    const tag = this.tagFilter;
    const out: Row[] = [];
    for (const r of this.rows) {
      if (this.viewMode === 'classic') {
        if (r.sep) continue;
        if (this.currentSep !== -2 && r.sepI !== this.currentSep) continue;
      } else if (!r.sep && r.sepI >= 0 && this.collapsed.has(this.rows[r.sepI].name) && !q) {
        continue;
      }
      if (q && !r.sep && !r.name.toLowerCase().includes(q)) continue;
      if (tag && !r.sep && !(r.u?.tags?.includes(tag) || r.u?.auto?.includes(tag))) continue;
      out.push(r);
    }
    return out;
  });

  private reloadTimer = 0;
  private unsub: (() => void) | null = null;

  async init() {
    try {
      this.status = await api.status();
      this.run = this.status.run;
      if (this.status.opened) await this.reload();
      else this.modal = 'start';
    } catch (e) {
      this.notify(String(e), 'err');
    }
    this.unsub = subscribe((ev) => {
      switch (ev.type) {
        case 'stage':
          if (this.run) {
            const s = ev.data as RunState['stages'][number];
            const i = this.run.stages.findIndex((x) => x.id === s.id);
            if (i >= 0) this.run.stages[i] = s;
          }
          break;
        case 'log':
          if (this.run) this.run.log = [...this.run.log.slice(-1500), ev.data as string];
          break;
        case 'run':
          this.run = ev.data as RunState;
          if (!this.run.active) this.scheduleReload(0);
          break;
        case 'changed':
        case 'opened':
          this.scheduleReload(60);
          break;
      }
    });
  }

  scheduleReload(ms = 80) {
    clearTimeout(this.reloadTimer);
    this.reloadTimer = window.setTimeout(() => this.reload(), ms);
  }

  async reload() {
    try {
      const [v, s] = await Promise.all([api.modlist(), api.summary()]);
      const prevGen = this.view?.gen;
      this.view = v;
      this.summary = s;
      if (v.gen !== prevGen) this.gen++;
      if (this.currentSep >= 0 && !v.rows[this.currentSep]?.sep) this.currentSep = -2;
      if (!this.focus) {
        const first = v.rows.find((r) => !r.sep);
        if (first && this.rightTab !== 'summary') this.focus = first.name;
      }
    } catch (e) {
      this.notify(String(e), 'err');
    }
  }

  notify(text: string, kind: 'ok' | 'err' | 'info' = 'info') {
    this.toast = { text: errText(text), kind };
    const t = this.toast;
    setTimeout(() => { if (this.toast === t) this.toast = null; }, kind === 'err' ? 6000 : 2600);
  }

  async act<T>(fn: () => Promise<T>, okText?: string): Promise<T | undefined> {
    try {
      const r = await fn();
      if (okText) this.notify(okText, 'ok');
      return r;
    } catch (e) {
      this.notify(String(e), 'err');
      return undefined;
    }
  }

  select(name: string, ev?: { ctrlKey?: boolean; metaKey?: boolean; shiftKey?: boolean }) {
    if (ev?.shiftKey && this.anchor) {
      const list = this.visibleRows.filter((r) => !r.sep).map((r) => r.name);
      const a = list.indexOf(this.anchor);
      const b = list.indexOf(name);
      if (a >= 0 && b >= 0) {
        if (!ev.ctrlKey && !ev.metaKey) this.selection.clear();
        for (let i = Math.min(a, b); i <= Math.max(a, b); i++) this.selection.add(list[i]);
        this.focus = name;
        return;
      }
    }
    if (ev?.ctrlKey || ev?.metaKey) {
      if (this.selection.has(name)) this.selection.delete(name);
      else this.selection.add(name);
    } else {
      this.selection.clear();
      this.selection.add(name);
    }
    this.anchor = name;
    this.focus = name;
  }

  async move(names: string[], before: string) {
    await this.act(() => api.move(names, before));
  }
  async toggle(names: string[], enabled?: boolean) {
    await this.act(() => api.toggle(names, enabled));
  }
  async undo() { await this.act(() => api.undo()); }
  async redo() { await this.act(() => api.redo()); }
  async save() {
    const r = await this.act(() => api.saveModlist());
    if (r) { this.notify(`modlist.txt сохранён · бэкап ${r.backup.split(/[\\/]/).pop()}`, 'ok'); this.reload(); }
  }
}

export const app = new AppState();

/** Monolith-style card background from a separator/mod color. */
export function colorToBg(color: string | undefined, enabled = true): string {
  const safe = (color || '').trim() || '#475569';
  const a1 = enabled ? 0.58 : 0.28;
  const a2 = enabled ? 0.38 : 0.18;
  return `linear-gradient(180deg, ${alpha(safe, a1)}, ${alpha(safe, a2)})`;
}
export function alpha(hex: string, a: number): string {
  const h = hex.replace('#', '');
  const f = h.length === 3 ? h.split('').map((c) => c + c).join('') : h;
  const n = parseInt(f.slice(0, 6), 16);
  if (Number.isNaN(n)) return `rgba(71,85,105,${a})`;
  return `rgba(${(n >> 16) & 255},${(n >> 8) & 255},${n & 255},${a})`;
}

export const errText = (e: unknown) => String(e).replace(/^Error: /, '');
