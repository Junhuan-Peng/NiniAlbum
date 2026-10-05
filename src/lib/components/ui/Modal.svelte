<script lang="ts">
  import { createEventDispatcher } from 'svelte';
  import { createDialog, melt } from '@melt-ui/svelte';

  export let title: string;
  export let description = '';
  export let width = 'max-w-md';
  const dispatch = createEventDispatcher<{ close: void }>();
  const dialog = createDialog({
    defaultOpen: true,
    onOpenChange: ({ next }) => {
      if (!next) dispatch('close');
      return next;
    }
  });
  const { overlay, content, title: titleElement, description: descriptionElement, close } = dialog.elements;
</script>

<div use:melt={$overlay} class="fixed inset-0 z-40 bg-black/70 backdrop-blur-[2px]"></div>
<div class="fixed inset-0 z-50 grid place-items-center p-5 pointer-events-none">
  <section use:melt={$content} class={`pointer-events-auto w-full ${width} rounded-xl border border-border bg-[#191b1f] p-5 shadow-2xl`}>
    <div class="mb-4 flex items-start justify-between gap-4">
      <div>
        <h2 use:melt={$titleElement} class="text-base font-semibold">{title}</h2>
        {#if description}<p use:melt={$descriptionElement} class="mt-1 text-sm text-muted">{description}</p>{/if}
      </div>
      <button use:melt={$close} class="rounded p-1 text-muted hover:bg-white/10 hover:text-white" aria-label="关闭">×</button>
    </div>
    <slot />
  </section>
</div>

