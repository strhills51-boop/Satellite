<script lang="ts">
  import type { ModStat } from '../lib/api';
  let { st, compact = false }: { st?: ModStat; compact?: boolean } = $props();
  const k = (n: number) => (n >= 10000 ? `${Math.round(n / 1000)}k` : n >= 1000 ? `${(n / 1000).toFixed(1)}k` : String(n));
</script>

{#if st && (st.win || st.lose || st.full)}
  <span class="strip" title="W — выигрывает файлов · L — проигрывает · M — в середине стека · X — полностью перекрыт · +n — уникальных">
    {#if st.full}<span class="chip x">X</span>{/if}
    {#if st.win}<span class="chip w">W {k(st.win)}</span>{/if}
    {#if st.lose}<span class="chip l">L {k(st.lose)}</span>{/if}
    {#if st.mid && !compact}<span class="chip m">M {k(st.mid)}</span>{/if}
    {#if st.unique && !compact}<span class="chip">+{k(st.unique)}</span>{/if}
  </span>
{/if}

<style>
  .strip { display: inline-flex; gap: 3px; flex: 0 0 auto; }
  .strip :global(.chip) { height: 18px; padding: 0 6px; font-size: 10.5px; }
</style>
