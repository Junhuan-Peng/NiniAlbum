<script lang="ts">
  import { createEventDispatcher } from 'svelte';

  export let title: string;
  export let description = '';
  export let width = 'max-w-md';
  const dispatch = createEventDispatcher<{ close: void }>();

  function close() {
    dispatch('close');
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') close();
  }
</script>

<svelte:window on:keydown={handleKeydown} />

<button class="fixed inset-0 z-40 cursor-default bg-black/70 backdrop-blur-[2px]" aria-label="关闭对话框" on:click={close}></button>
<div class="pointer-events-none fixed inset-0 z-50 grid place-items-center p-5">
  <section role="dialog" aria-modal="true" aria-label={title} class={`pointer-events-auto w-full ${width} max-h-[calc(100vh-2.5rem)] overflow-y-auto rounded-xl border border-border bg-[#191b1f] p-5 shadow-2xl`}>
    <div class="mb-4 flex items-start justify-between gap-4">
      <div>
        <h2 class="text-base font-semibold">{title}</h2>
        {#if description}<p class="mt-1 text-sm text-muted">{description}</p>{/if}
      </div>
      <button class="rounded p-1 text-muted hover:bg-white/10 hover:text-white" aria-label="关闭" on:click={close}>×</button>
    </div>
    <slot></slot>
  </section>
</div>
