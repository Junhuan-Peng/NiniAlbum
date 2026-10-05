<script lang="ts">
  import type { TagNode } from '../types';

  export let tag: TagNode;
  export let tags: TagNode[];
  export let selectedId: number | null;
  export let assignedId: number | null;
  export let depth = 0;
  export let onSelect: (id: number) => void;
  let expanded = true;
  $: children = tags.filter((item) => item.parentId === tag.id);
</script>

<div>
  <div class="group flex items-center gap-1 pr-2" style={`padding-left:${depth * 14 + 4}px`}>
    <button class="grid h-7 w-5 place-items-center text-xs text-muted" on:click={() => expanded = !expanded} aria-label={expanded ? '折叠' : '展开'}>
      {children.length ? (expanded ? '▾' : '▸') : ''}
    </button>
    <button
      class:selected={selectedId === tag.id}
      class:assigned={assignedId === tag.id}
      class="flex min-w-0 flex-1 items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-white/5"
      on:click={() => onSelect(tag.id)}
    >
      <span class="h-1.5 w-1.5 shrink-0 rounded-full bg-current opacity-40"></span>
      <span class="truncate">{tag.name}</span>
      {#if tag.photoCount}<span class="ml-auto text-[10px] tabular-nums text-muted">{tag.photoCount}</span>{/if}
    </button>
  </div>
  {#if expanded}
    {#each children as child (child.id)}
      <svelte:self tag={child} {tags} {selectedId} {assignedId} depth={depth + 1} {onSelect} />
    {/each}
  {/if}
</div>

<style>
  .selected { background: rgba(217, 255, 91, .09); color: #d9ff5b; }
  .assigned { box-shadow: inset 2px 0 #d9ff5b; }
</style>

