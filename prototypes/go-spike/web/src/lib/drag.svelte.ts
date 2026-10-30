// Shared drag state (HTML5 DnD dataTransfer is not readable during dragover).
class DragState {
  kind = $state<'' | 'mod' | 'sep'>('');
  names = $state<string[]>([]);
  start(e: DragEvent, kind: 'mod' | 'sep', names: string[]) {
    this.kind = kind;
    this.names = names;
    e.dataTransfer?.setData('text/plain', names.join('\n'));
    if (e.dataTransfer) e.dataTransfer.effectAllowed = 'move';
  }
  end() {
    this.kind = '';
    this.names = [];
  }
}
export const drag = new DragState();
