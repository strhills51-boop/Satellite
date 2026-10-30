// Typed client for the local engine API.

export interface ModStat {
  files: number; unique: number; win: number; lose: number; mid: number; full: boolean;
  chains: number; context: number; vramWinKiB: number; bytesWin: number;
}
export interface ModUser {
  tags?: string[]; stickers?: string[]; auto?: string[]; note?: string; color?: string; emoji?: string; locked?: boolean;
}
export interface Row {
  i: number; name: string; display: string; sep?: boolean; en: boolean; unm?: boolean; sepI: number;
  prio?: number; color?: string; st?: ModStat; missing?: boolean; size?: number; plg?: number; u?: ModUser; ver?: string;
}
export interface Instance {
  iniPath: string; baseDir: string; gameName: string; modsDir: string; extraModDirs: string[];
  overwriteDir: string; profilesDir: string; profile: string; profiles: string[];
}
export interface ModlistView {
  profile: string; instance: Instance | null; rows: Row[]; dirty: boolean; analyzed: boolean; lastScan: string;
  gen: number; canUndo: boolean; canRedo: boolean; overwrite?: Row;
}
export interface Metrics {
  bytes: number; files: number; dirs: number; rootFiles: number; maxDepth: number; dds: number; ddsKnown: number;
  vramBytes: number; nif: number; pex: number; hkx: number; dll: number; plugins: number; archives: number;
  archiveFiles: number; nestedArchives: number; sourceFiles: number; ext: Record<string, number>;
  topDirs: Record<string, number>; ddsFormats?: Record<string, number>;
}
export interface Plugin {
  name: string; master: boolean; light: boolean; version: number; numRecords: number; masters: string[];
  author?: string; records?: Record<string, number>; recordTotal?: number; error?: string;
}
export interface ArchiveRef { mod: number; name: string; kind: string; entries: number; error?: string }
export interface ModDetails {
  name: string; path: string; meta: Record<string, string>; metrics?: Metrics; plugins?: Plugin[];
  archives?: ArchiveRef[]; stat?: ModStat; user?: ModUser; scanned: string; cached: boolean; missing: boolean; error?: string;
}
export interface ChainView {
  id: number; key: string; mods: string[]; files: number; bytes: number; context: number;
  done: boolean; label?: string; color?: string; icon?: string; role?: 'W' | 'L' | 'M';
}
export interface Provider { mod: string; archive?: string; size: number; vramKiB?: number; active: boolean; winner: boolean }
export interface PathView { path: string; stack: Provider[] }
export interface Page<T> { total: number; offset: number; items: T[] }
export interface ContextView { id: number; mods: string[]; files: number; chains: ChainView[]; done: number }
export interface TreeEntry { name: string; dir: boolean; files?: number; size: number; archive?: string; state?: string; others?: number; path?: string }
export interface PluginView {
  name: string; enabled: boolean; index: number; slot?: string; kind: string; masters?: string[];
  missingMasters?: string[]; lateMasters?: string[]; owner?: string; owners?: string[]; found: boolean;
  inLoadorder: boolean; records?: number; refs?: number; author?: string; error?: string;
}
export interface Stage { id: string; title: string; status: string; text: string; percent: number }
export interface RunState { active: boolean; startedAt: string; endedAt: string; stages: Stage[]; log: string[]; error?: string }
export interface Summary {
  Mods: number; Enabled: number; Disabled: number; Separators: number; Bytes: number; Files: number; Paths: number;
  ConflictPaths: number; Chains: number; Contexts: number; VRAMKnownBytes: number; VRAMWinBytes: number;
  Plugins: number; PluginsActive: number; ESM: number; ESP: number; ESL: number; FullSlots: number;
  DDS: number; NIF: number; PEX: number; HKX: number; DLL: number; Archives: number; FullyOverridden: number;
  LastScan: string; Workspace: string;
}
export interface JournalEntry { at: string; type: string; text: string }
export interface ScanOptions { force: boolean; dds: boolean; archives: boolean; plugins: boolean; records: boolean; onlyActive: boolean; workers?: number }
export interface Status {
  version: string; opened: boolean; os: string; run: RunState;
  config: { instance: string; extra: string[] | null; profile: string; recent: string[] | null };
}
export interface FsList { path: string; parent: string; entries: { name: string; path: string; dir: boolean; isMO2?: boolean }[] }

async function req<T>(method: string, url: string, body?: unknown): Promise<T> {
  const r = await fetch(url, {
    method,
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body ? JSON.stringify(body) : undefined,
  });
  const data = await r.json().catch(() => ({ error: `HTTP ${r.status}` }));
  if (!r.ok) throw new Error(data?.error || `HTTP ${r.status}`);
  return data as T;
}
const q = (o: Record<string, string | number | undefined>) =>
  Object.entries(o).filter(([, v]) => v !== undefined && v !== '').map(([k, v]) => `${k}=${encodeURIComponent(String(v))}`).join('&');

export const api = {
  status: () => req<Status>('GET', '/api/status'),
  open: (path: string, extra: string[], profile = '') => req<ModlistView>('POST', '/api/open', { path, extra, profile }),
  modlist: () => req<ModlistView>('GET', '/api/modlist'),
  summary: () => req<Summary>('GET', '/api/summary'),
  journal: () => req<JournalEntry[]>('GET', '/api/journal'),
  tags: () => req<Record<string, number>>('GET', '/api/tags'),
  plugins: () => req<PluginView[]>('GET', '/api/plugins'),
  run: () => req<RunState>('GET', '/api/run'),
  scan: (o: ScanOptions) => req<{ ok: boolean }>('POST', '/api/scan', o),
  move: (names: string[], before: string) => req('POST', '/api/move', { names, before }),
  toggle: (names: string[], enabled?: boolean) => req('POST', '/api/toggle', { names, enabled }),
  undo: () => req<{ ok: boolean }>('POST', '/api/undo'),
  redo: () => req<{ ok: boolean }>('POST', '/api/redo'),
  saveModlist: () => req<{ backup: string }>('POST', '/api/save-modlist'),
  user: (p: { kind: string; names?: string[]; key?: string; patch?: unknown; addTags?: string[]; removeTags?: string[] }) =>
    req('POST', '/api/user', p),
  mod: (name: string) => req<ModDetails>('GET', `/api/mod?${q({ name })}`),
  modChains: (name: string) => req<ChainView[] | null>('GET', `/api/mod/chains?${q({ name })}`),
  tree: (name: string, dir: string) => req<TreeEntry[]>('GET', `/api/mod/tree?${q({ name, dir })}`),
  chainFiles: (id: number, filter = '', offset = 0, limit = 200) =>
    req<Page<PathView>>('GET', `/api/chain/files?${q({ id, q: filter, offset, limit })}`),
  contexts: () => req<ContextView[] | null>('GET', '/api/contexts'),
  context: (id: number) => req<ContextView>('GET', `/api/context?${q({ id })}`),
  search: (s: string, offset = 0, limit = 200) => req<Page<PathView>>('GET', `/api/search?${q({ q: s, offset, limit })}`),
  reveal: (name: string) => req('POST', '/api/reveal', { name }),
  fsList: (path: string) => req<FsList>('GET', `/api/fs/list?${q({ path })}`),
};

export type EngineEvent = { type: string; data?: unknown };

export function subscribe(fn: (e: EngineEvent) => void): () => void {
  let es: EventSource | null = null;
  let closed = false;
  const connect = () => {
    es = new EventSource('/api/events');
    es.onmessage = (m) => { try { fn(JSON.parse(m.data)); } catch { /* ignore */ } };
    es.onerror = () => { es?.close(); if (!closed) setTimeout(connect, 1500); };
  };
  connect();
  return () => { closed = true; es?.close(); };
}

export function fmtBytes(n: number | undefined): string {
  if (!n) return '0 B';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  let v = n;
  while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; }
  return `${v.toFixed(v >= 100 || i === 0 ? 0 : 1)} ${u[i]}`;
}
export const fmtN = (n: number | undefined) => (n ?? 0).toLocaleString('ru-RU');
