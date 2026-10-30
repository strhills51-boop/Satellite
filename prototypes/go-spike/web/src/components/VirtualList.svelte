<script lang="ts" generics="T">
  import type { Snippet } from 'svelte';

  let {
    items,
    itemHeight,
    row,
    overscan = 8,
    scrollToIndex = -1,
    onscroll,
  }: {
    items: T[];
    itemHeight: number;
    row: Snippet<[T, number]>;
    overscan?: number;
    scrollToIndex?: number;
    onscroll?: (el: HTMLDivElement) => void;
  } = $props();

  let el: HTMLDivElement;
  let scrollTop = $state(0);
  let height = $state(600);

  const start = $derived(Math.max(0, Math.floor(scrollTop / itemHeight) - overscan));
  const end = $derived(Math.min(items.length, Math.ceil((scrollTop + height) / itemHeight) + overscan));
  const slice = $derived(items.slice(start, end));

  $effect(() => {
    if (scrollToIndex < 0 || !el) return;
    const top = scrollToIndex * itemHeight;
    if (top < el.scrollTop || top + itemHeight > el.scrollTop + el.clientHeight) {
      el.scrollTop = Math.max(0, top - el.clientHeight / 3);
    }
  });

  export function getElement() { return el; }
</script>

<div
  class="vl"
  bind:this={el}
  bind:clientHeight={height}
  onscroll={() => { scrollTop = el.scrollTop; onscroll?.(el); }}
>
  <div style="height:{items.length * itemHeight}px; position:relative">
    <div style="position:absolute; left:0; right:0; top:{start * itemHeight}px">
      {#each slice as item, k (start + k)}
        <div style="height:{itemHeight}px">{@render row(item, start + k)}</div>
      {/each}
    </div>
  </div>
</div>

<style>
  .vl { height: 100%; overflow: auto; position: relative; }
</style>
