<script lang="ts">
  export let variant: 'primary' | 'secondary' | 'ghost' | 'danger' = 'secondary';
  export let size: 'sm' | 'md' = 'md';
  export let disabled = false;
  export let type: 'button' | 'submit' = 'button';
  let className = '';
  export { className as class };

  $: classes = [
    'inline-flex items-center justify-center gap-2 rounded-md border font-medium transition focus:outline-none focus:ring-2 focus:ring-accent/50 disabled:cursor-not-allowed disabled:opacity-45',
    size === 'sm' ? 'h-8 px-2.5 text-xs' : 'h-9 px-3.5 text-sm',
    variant === 'primary' && 'border-accent bg-accent text-black hover:bg-[#e5ff83]',
    variant === 'secondary' && 'border-border bg-[#22252a] hover:border-[#444952] hover:bg-[#292d33]',
    variant === 'ghost' && 'border-transparent bg-transparent text-muted hover:bg-white/5 hover:text-foreground',
    variant === 'danger' && 'border-danger/50 bg-danger/10 text-danger hover:bg-danger/20',
    className
  ].filter(Boolean).join(' ');
</script>

<button {type} {disabled} class={classes} on:click on:pointerdown><slot /></button>

