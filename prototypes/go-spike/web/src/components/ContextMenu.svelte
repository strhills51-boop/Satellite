<script lang="ts" module>
  export interface MenuItem {
    label?: string;
    icon?: string;
    action?: () => void;
    children?: MenuItem[];
    divider?: boolean;
    disabled?: boolean;
    color?: string;
  }
  class MenuState {
    open = $state(false);
    x = $state(0);
    y = $state(0);
    items = $state<MenuItem[]>([]);
    show(e: MouseEvent, items: MenuItem[]) {
      e.preventDefault();
      this.items = items;
      this.x = e.clientX;
      this.y = e.clientY;
      this.open = true;
    }
    close() { this.open = false; }
  }
  export const menu = new MenuState();
</script>

<script lang="ts">
  let el = $state<HTMLDivElement>();
  let sub = $state<number>(-1);
  let pos = $state({ x: 0, y: 0 });

  $effect(() => {
    if (!menu.open || !el) return;
    sub = -1;
    const r = el.getBoundingClientRect();
    pos = {
      x: Math.min(menu.x, window.innerWidth - r.width - 8),
      y: Math.min(menu.y, window.innerHeight - r.height - 8),
    };
  });

  function run(it: MenuItem) {
    if (it.disabled || it.children) return;
    menu.close();
    it.action?.();
  }
</script>

<svelte:window
  onkeydown={(e) => e.key === 'Escape' && menu.close()}
  onmousedown={(e) => { if (menu.open && el && !el.contains(e.target as Node)) menu.close(); }}
  onblur={() => menu.close()}
/>

{#if menu.open}
  <div class="ctx" bind:this={el} style="left:{pos.x}px; top:{pos.y}px">
    {#each menu.items as it, i}
      {#if it.divider}
        <div class="div"></div>
      {:else}
        <div
          class="it"
          class:dis={it.disabled}
          role="menuitem"
          tabindex="-1"
          onmouseenter={() => (sub = it.children ? i : -1)}
          onclick={() => run(it)}
          onkeydown={(e) => e.key === 'Enter' && run(it)}
        >
          <span class="ic">{#if it.color}<i style="background:{it.color}"></i>{:else}{it.icon ?? ''}{/if}</span>
          <span class="grow">{it.label}</span>
          {#if it.children}<span class="muted">›</span>{/if}
          {#if it.children && sub === i}
            <div class="ctx subm">
              {#each it.children as c}
                {#if c.divider}<div class="div"></div>{:else}
                  <div class="it" role="menuitem" tabindex="-1" onclick={(e) => { e.stopPropagation(); run(c); }} onkeydown={() => {}}>
                    <span class="ic">{#if c.color}<i style="background:{c.color}"></i>{:else}{c.icon ?? ''}{/if}</span>
                    <span class="grow ellipsis">{c.label}</span>
                  </div>
                {/if}
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/if}

<style>
  .ctx {
    position: fixed; z-index: 400; min-width: 220px; max-width: 360px; border-radius: 12px; border: 1px solid var(--line);
    background: var(--pop); box-shadow: var(--shadow); padding: 6px;
  }
  .subm { position: absolute; left: calc(100% - 4px); top: -6px; max-height: 60vh; overflow: auto; }
  .it { position: relative; display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 8px; cursor: pointer; font-size: 13px; }
  .it:hover { background: #2a3647; }
  .it.dis { opacity: .45; cursor: default; }
  .ic { width: 18px; text-align: center; flex: 0 0 auto; }
  .ic i { display: inline-block; width: 12px; height: 12px; border-radius: 4px; vertical-align: middle; }
  .div { height: 1px; background: var(--line); margin: 5px 4px; }
</style>
