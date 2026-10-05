<script lang="ts">
  import { dismissToast, toasts } from '../../toast';
</script>

<div class="pointer-events-none fixed bottom-12 right-4 z-[80] flex w-80 flex-col gap-2" aria-live="polite">
  {#each $toasts as item (item.id)}
    <button
      class:border-danger={item.tone === 'error'}
      class:border-emerald-400={item.tone === 'success'}
      class="pointer-events-auto rounded-lg border border-border bg-[#24272c] px-4 py-3 text-left text-sm shadow-xl"
      on:click={() => dismissToast(item.id)}
    >{item.message}</button>
  {/each}
</div>

