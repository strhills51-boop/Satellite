import { api, type Row } from './api';
import { app } from './store.svelte';
import type { MenuItem } from '../components/ContextMenu.svelte';

export const PALETTE = ['#2457a8', '#0e7490', '#15803d', '#65a30d', '#ca8a04', '#c2410c', '#b91c1c', '#be185d', '#7e22ce', '#4338ca', '#475569', '#0f766e'];

/** Name of the entry that follows the block of the separator at row index i ("" = end). */
export function afterSeparatorBlock(sepIndex: number): string {
  const rows = app.rows;
  for (let j = sepIndex + 1; j < rows.length; j++) if (rows[j].sep) return rows[j].name;
  return '';
}

export function selectedOr(name: string): string[] {
  return app.selection.has(name) ? [...app.selection] : [name];
}

export function modMenu(r: Row): MenuItem[] {
  const names = selectedOr(r.name);
  const many = names.length > 1;
  return [
    { label: many ? `Включить (${names.length})` : 'Включить', icon: '●', action: () => app.toggle(names, true) },
    { label: many ? `Выключить (${names.length})` : 'Выключить', icon: '○', action: () => app.toggle(names, false) },
    { divider: true },
    {
      label: 'Перенести в разделитель', icon: '↪',
      children: app.seps.map((s) => ({ label: s.display, color: s.color, action: () => app.move(names, afterSeparatorBlock(s.i)) })),
    },
    { label: 'В начало списка', icon: '⤒', action: () => app.move(names, app.rows[0]?.name ?? '') },
    { label: 'В конец списка', icon: '⤓', action: () => app.move(names, '') },
    { divider: true },
    { label: 'Конфликты', icon: '⚔', action: () => { app.focus = r.name; app.rightTab = 'conflicts'; } },
    { label: 'Дерево файлов', icon: '🌲', action: () => { app.focus = r.name; app.rightTab = 'tree'; } },
    { label: 'Открыть папку', icon: '📂', action: () => app.act(() => api.reveal(r.name)) },
    { label: 'Копировать имя', icon: '⧉', action: () => navigator.clipboard?.writeText(names.join('\n')) },
    { divider: true },
    {
      label: 'Покрасить…', icon: '🎨',
      children: [
        ...PALETTE.map((c) => ({ label: c, color: c, action: () => app.act(() => api.user({ kind: 'mod', names, patch: { color: c } })) })),
        { divider: true },
        { label: 'Цвет разделителя', icon: '↺', action: () => app.act(() => api.user({ kind: 'mod', names, patch: { color: '' } })) },
      ],
    },
  ];
}

export function sepMenu(s: Row): MenuItem[] {
  const isCollapsed = app.collapsed.has(s.name);
  return [
    { label: 'Показать моды разделителя', icon: '☰', action: () => { app.currentSep = s.i; app.viewMode = 'classic'; } },
    { label: isCollapsed ? 'Развернуть' : 'Свернуть', icon: isCollapsed ? '▸' : '▾', action: () => (isCollapsed ? app.collapsed.delete(s.name) : app.collapsed.add(s.name)) },
    { label: 'Свернуть все разделители', icon: '⊟', action: () => app.seps.forEach((x) => app.collapsed.add(x.name)) },
    { label: 'Раскрыть все разделители', icon: '⊞', action: () => app.collapsed.clear() },
    { divider: true },
    { label: 'Включить все моды', icon: '●', action: () => app.toggle(modsOf(s.i), true) },
    { label: 'Выключить все моды', icon: '○', action: () => app.toggle(modsOf(s.i), false) },
    { divider: true },
    {
      label: 'Покрасить…', icon: '🎨',
      children: [
        ...PALETTE.map((c) => ({ label: c, color: c, action: () => app.act(() => api.user({ kind: 'sep', names: [s.name], patch: { color: c } })) })),
        { divider: true },
        { label: 'Цвет из MO2', icon: '↺', action: () => app.act(() => api.user({ kind: 'sep', names: [s.name], patch: { color: '' } })) },
      ],
    },
    { label: 'Открыть папку', icon: '📂', action: () => app.act(() => api.reveal(s.name)) },
  ];
}

export function modsOf(sepIndex: number): string[] {
  return app.rows.filter((r) => !r.sep && r.sepI === sepIndex).map((r) => r.name);
}
