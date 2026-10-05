<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { listen } from '@tauri-apps/api/event';
  import { open, save } from '@tauri-apps/plugin-dialog';
  import Button from './lib/components/ui/Button.svelte';
  import Modal from './lib/components/ui/Modal.svelte';
  import ToastViewport from './lib/components/ui/ToastViewport.svelte';
  import TagBranch from './lib/components/TagBranch.svelte';
  import { api } from './lib/api';
  import { toast } from './lib/toast';
  import type { DeleteImpact, ExportPlan, PhotoEntry, PhotoFilter, ProjectSnapshot, ScanProgress, SortDirection, SortField, TagNode } from './lib/types';

  let snapshot: ProjectSnapshot | null = null;
  let selectedIndex = 0;
  let selectedTagId: number | null = null;
  let filter: PhotoFilter = 'all';
  let sortField: SortField = 'file_name';
  let sortDirection: SortDirection = 'asc';
  let busy = false;
  let busyLabel = '';
  let previewData: string | null = null;
  let previewLoading = false;
  let scanProgress: ScanProgress | null = null;
  let zoom = 1;
  let leftWidth = 248;
  let rightWidth = 310;
  let tagDialog: 'root' | 'child' | 'rename' | null = null;
  let tagName = '';
  let deleteDialog: { tag: TagNode; impact: DeleteImpact } | null = null;
  let exportDialog: { root: string; plan: ExportPlan; strategy: 'skip' | 'overwrite' | 'rename' } | null = null;
  let recentProjects: { path: string; name: string; photoRoot: string; lastOpened: number }[] = [];
  let smartKind: 'none' | 'date' | 'location' | 'similarity' = 'none';
  let smartGroup = '';

  $: sourcePhotos = snapshot?.photos ?? [];
  $: smartOptions = Array.from(new Set(sourcePhotos.map((photo) => smartValue(photo, smartKind)).filter((value): value is string => Boolean(value)))).sort();
  $: photos = smartKind === 'none'
    ? sourcePhotos
    : sourcePhotos.filter((photo) => smartGroup ? smartValue(photo, smartKind) === smartGroup : Boolean(smartValue(photo, smartKind)));
  $: currentPhoto = photos[selectedIndex] ?? null;
  $: roots = (snapshot?.tags ?? []).filter((tag) => tag.parentId === null);
  $: selectedTag = snapshot?.tags.find((tag) => tag.id === selectedTagId) ?? null;
  $: scanPercent = scanProgress && scanProgress.total > 0
    ? Math.min(100, Math.round((scanProgress.completed / scanProgress.total) * 100))
    : 0;

  const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);

  function smartValue(photo: PhotoEntry, kind: typeof smartKind) {
    if (kind === 'date') return photo.dateGroup;
    if (kind === 'location') return photo.locationGroup;
    if (kind === 'similarity') return photo.similarityGroup;
    return null;
  }

  function projectName(path: string) {
    return path.split(/[\\/]/).pop()?.replace(/\.sqlite$/i, '') || '未命名项目';
  }

  function rememberProject(path: string, photoRoot: string) {
    const item = { path, name: projectName(path), photoRoot, lastOpened: Date.now() };
    recentProjects = [item, ...recentProjects.filter((recent) => recent.path !== path)].slice(0, 6);
    localStorage.setItem('nini-albums-recent-projects', JSON.stringify(recentProjects));
  }

  async function changeSmartKind() {
    smartGroup = '';
    selectedIndex = 0;
    await tick();
    await loadPreview(currentPhoto);
  }

  async function changeSmartGroup() {
    selectedIndex = 0;
    await tick();
    await loadPreview(currentPhoto);
  }

  async function runBusy<T>(label: string, task: () => Promise<T>): Promise<T | undefined> {
    busy = true; busyLabel = label;
    try { return await task(); }
    catch (error) { toast(errorMessage(error), 'error', 6000); }
    finally { busy = false; busyLabel = ''; }
  }

  async function refresh(preserveId = currentPhoto?.id) {
    const result = await api.snapshot({ filter, sortField, sortDirection });
    snapshot = result;
    sortField = result.project.sortField;
    sortDirection = result.project.sortDirection;
    await tick();
    const next = preserveId ? photos.findIndex((photo) => photo.id === preserveId) : 0;
    selectedIndex = next >= 0 ? next : Math.min(selectedIndex, Math.max(0, result.photos.length - 1));
    await tick();
    await loadPreview(currentPhoto);
  }

  async function createProject() {
    const photoRoot = await open({ directory: true, multiple: false, title: '选择照片根目录' });
    if (!photoRoot || Array.isArray(photoRoot)) return;
    let databasePath = await save({ title: '保存项目数据库', defaultPath: 'nini-albums.sqlite', filters: [{ name: 'SQLite 项目', extensions: ['sqlite'] }] });
    if (!databasePath) return;
    if (!databasePath.toLowerCase().endsWith('.sqlite')) databasePath += '.sqlite';
    await runBusy('正在创建项目…', async () => {
      await api.createProject(databasePath!, photoRoot);
      filter = 'all'; selectedIndex = 0; await refresh();
      rememberProject(databasePath!, photoRoot);
      toast('项目已创建，可以开始扫描照片。', 'success');
    });
  }

  async function openProject() {
    const path = await open({ multiple: false, directory: false, title: '打开项目数据库', filters: [{ name: 'SQLite 项目', extensions: ['sqlite'] }] });
    if (!path || Array.isArray(path)) return;
    await runBusy('正在打开项目…', async () => {
      const info = await api.openProject(path);
      filter = 'all'; sortField = info.sortField; sortDirection = info.sortDirection; selectedIndex = 0;
      await refresh();
      rememberProject(path, info.photoRoot);
      toast(info.photoRootAvailable ? '项目已打开。' : '项目已打开，但原照片目录不可用，请重新定位。', info.photoRootAvailable ? 'success' : 'error', 6000);
    });
  }

  async function openRecentProject(path: string) {
    await runBusy('正在打开项目…', async () => {
      const info = await api.openProject(path);
      filter = 'all'; smartKind = 'none'; smartGroup = ''; sortField = info.sortField; sortDirection = info.sortDirection; selectedIndex = 0;
      await refresh();
      rememberProject(path, info.photoRoot);
      toast(info.photoRootAvailable ? '项目已打开。' : '原照片目录不可用，请重新定位。', info.photoRootAvailable ? 'success' : 'error');
    });
  }

  async function scanPhotos() {
    scanProgress = { phase: 'discovering', completed: 0, total: 0, message: '正在递归发现照片文件' };
    try {
      await runBusy('正在扫描…', async () => {
        const result = await api.scan(); await refresh();
        toast(`扫描完成：${result.totalGroups} 组，完整 ${result.complete}，缺配对 ${result.missingJpeg + result.missingRaw}，冲突 ${result.conflicts}`, result.warnings.length ? 'default' : 'success', 6000);
      });
    } finally {
      scanProgress = null;
    }
  }

  async function relocatePhotoRoot() {
    if (!snapshot) return;
    const root = await open({ directory: true, multiple: false, title: '重新定位照片根目录' });
    if (!root || Array.isArray(root)) return;
    try { await api.setPhotoRoot(root); await refresh(); toast('照片根目录已更新，请重新扫描。', 'success'); }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function loadPreview(photo: PhotoEntry | null) {
    previewData = null; zoom = 1;
    if (!photo?.jpegRelativePath || photo.isMissing || photo.pairStatus === 'conflict') return;
    const requested = photo.id; previewLoading = true;
    try { const data = await api.preview(requested); if (currentPhoto?.id === requested) previewData = data; }
    catch (error) { toast(`预览加载失败：${errorMessage(error)}`, 'error'); }
    finally { previewLoading = false; }
  }

  async function choosePhoto(index: number) {
    if (index < 0 || index >= photos.length) return;
    selectedIndex = index; await tick(); await loadPreview(currentPhoto);
  }

  async function assignSelectedTag(tagId: number, advance = false) {
    if (!currentPhoto) return;
    selectedTagId = tagId;
    const photoId = currentPhoto.id;
    try {
      await api.assignTag(photoId, tagId); await refresh(photoId);
      if (advance) await choosePhoto(Math.min(selectedIndex + 1, photos.length - 1));
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  function startTagDialog(mode: 'root' | 'child' | 'rename') {
    if ((mode === 'child' || mode === 'rename') && !selectedTag) { toast('请先选择一个标签。'); return; }
    tagName = mode === 'rename' ? selectedTag?.name ?? '' : ''; tagDialog = mode;
  }

  async function submitTag() {
    if (!tagDialog || !tagName.trim()) return;
    try {
      if (tagDialog === 'rename' && selectedTag) await api.renameTag(selectedTag.id, tagName);
      else await api.createTag(tagName, tagDialog === 'child' ? selectedTagId : null);
      tagDialog = null; await refresh(); toast('标签已保存。', 'success');
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function inspectDelete() {
    if (!selectedTag) { toast('请先选择标签。'); return; }
    try { deleteDialog = { tag: selectedTag, impact: await api.deleteImpact(selectedTag.id) }; }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function confirmDelete() {
    if (!deleteDialog) return;
    const unassign = deleteDialog.impact.affectedPhotos > 0;
    try { await api.deleteTag(deleteDialog.tag.id, unassign); deleteDialog = null; selectedTagId = null; await refresh(); toast('标签已删除。', 'success'); }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function updateQuery() {
    if (!snapshot) return;
    await runBusy('正在更新列表…', async () => { await api.saveSort(sortField, sortDirection); await refresh(); });
  }

  async function deleteCurrentMissing() {
    if (!currentPhoto?.isMissing || !confirm(`仅删除“${currentPhoto.displayBaseName}”的数据库记录？不会删除磁盘文件。`)) return;
    try { await api.deleteMissing([currentPhoto.id]); await refresh(); toast('丢失记录已删除。', 'success'); }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function prepareExport() {
    const root = await open({ directory: true, multiple: false, title: '选择导出根目录' });
    if (!root || Array.isArray(root)) return;
    await runBusy('正在检查导出内容…', async () => {
      const plan = await api.planExport(root); exportDialog = { root, plan, strategy: plan.conflicts ? 'skip' : 'overwrite' };
    });
  }

  async function confirmExport() {
    if (!exportDialog) return;
    const { root, strategy } = exportDialog; exportDialog = null;
    await runBusy('正在复制 JPEG 与 ARW…', async () => {
      const result = await api.exportPhotos(root, strategy);
      toast(`导出完成：成功 ${result.succeeded}，跳过 ${result.skipped}，失败 ${result.failed}${result.renamed ? `，改名 ${result.renamed}` : ''}`, result.failed ? 'error' : 'success', 7000);
    });
  }

  async function syncProject() {
    if (!snapshot) return;
    try { await api.checkpoint(); toast('项目数据已同步到磁盘。', 'success'); }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  function handleKeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (['INPUT','TEXTAREA','SELECT'].includes(target.tagName) || target.isContentEditable) return;
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 's') { event.preventDefault(); syncProject(); return; }
    if (!snapshot || tagDialog || deleteDialog || exportDialog) return;
    if (event.key === 'ArrowDown') { event.preventDefault(); choosePhoto(Math.min(selectedIndex + 1, photos.length - 1)); }
    if (event.key === 'ArrowUp') { event.preventDefault(); choosePhoto(Math.max(selectedIndex - 1, 0)); }
    if (event.code === 'Space' && selectedTagId !== null) { event.preventDefault(); assignSelectedTag(selectedTagId, true); }
  }

  function resize(side: 'left' | 'right', event: PointerEvent) {
    const startX = event.clientX; const initial = side === 'left' ? leftWidth : rightWidth;
    const move = (next: PointerEvent) => {
      const delta = next.clientX - startX;
      if (side === 'left') leftWidth = Math.max(190, Math.min(420, initial + delta));
      else rightWidth = Math.max(240, Math.min(460, initial - delta));
    };
    const stop = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', stop); };
    window.addEventListener('pointermove', move); window.addEventListener('pointerup', stop);
  }

  function zoomPreview(event: WheelEvent) {
    if (!previewData) return;
    event.preventDefault(); zoom = Math.max(.2, Math.min(6, zoom * (event.deltaY < 0 ? 1.12 : .89)));
  }

  function focusOnMount(node: HTMLElement) {
    queueMicrotask(() => node.focus());
  }

  onMount(() => {
    try {
      const stored = JSON.parse(localStorage.getItem('nini-albums-recent-projects') ?? '[]');
      if (Array.isArray(stored)) recentProjects = stored.slice(0, 6);
    } catch { recentProjects = []; }
    let unlisten: (() => void) | undefined;
    listen<ScanProgress>('scan-progress', ({ payload }) => {
      if (scanProgress?.phase === payload.phase && scanProgress.message === payload.message
        && payload.completed < scanProgress.completed) return;
      scanProgress = payload;
      busyLabel = payload.total > 0
        ? `${payload.message} ${payload.completed}/${payload.total}`
        : payload.message;
    }).then((dispose) => { unlisten = dispose; }).catch(() => { /* Browser-only preview has no Tauri event bridge. */ });
    return () => unlisten?.();
  });
</script>

<svelte:window on:keydown={handleKeydown} />

<main class="flex h-full min-h-0 flex-col bg-background text-foreground">
  <header class="flex min-h-16 shrink-0 items-center gap-4 border-b border-border bg-[#15171a] px-4 py-2">
    <div class="flex min-w-[210px] shrink-0 items-center gap-3"><div class="grid h-9 w-9 place-items-center rounded-xl bg-accent font-black text-black">N</div><div class="min-w-0 leading-tight"><strong class="block text-sm">Nini Albums</strong><span class="block max-w-52 truncate text-[10px] text-muted">{snapshot?.project.databasePath ?? '尚未打开项目'}</span></div></div>
    <div class="flex min-w-0 flex-1 items-center gap-3 overflow-x-auto pb-0.5">
      <div class="flex shrink-0 items-center gap-2 rounded-xl border border-border bg-black/15 p-1"><span class="px-1.5 text-[9px] font-semibold uppercase tracking-widest text-muted">项目</span><Button size="sm" on:click={createProject}>新建</Button><Button size="sm" on:click={openProject}>打开</Button></div>
      <div class="flex shrink-0 items-center gap-2 rounded-xl border border-border bg-black/15 p-1"><span class="px-1.5 text-[9px] font-semibold uppercase tracking-widest text-muted">照片</span><Button size="sm" disabled={!snapshot || busy} on:click={scanPhotos}>递归扫描</Button><Button size="sm" variant="ghost" disabled={!snapshot || busy} on:click={relocatePhotoRoot}>照片目录</Button><Button size="sm" variant="primary" disabled={!snapshot || busy} on:click={prepareExport}>批量导出</Button></div>
      <Button class="shrink-0" size="sm" variant="ghost" disabled={!snapshot} on:click={syncProject}>同步</Button>
    </div>
    <div class="ml-auto flex shrink-0 items-center gap-2 text-xs text-muted">{#if busy}<span class="inline-block h-3 w-3 animate-spin rounded-full border-2 border-accent border-r-transparent"></span><span class="max-w-48 truncate">{busyLabel}</span>{/if}</div>
  </header>

  {#if busy && scanProgress}
    <div class="relative flex h-8 shrink-0 items-center justify-between overflow-hidden border-b border-border bg-[#121417] px-3 text-[11px] text-muted">
      <div
        class:animate-pulse={scanProgress.total === 0}
        class="absolute inset-y-0 left-0 bg-accent/10 transition-[width] duration-200"
        style={`width:${scanProgress.total > 0 ? scanPercent : 100}%`}
      ></div>
      <span class="relative">{scanProgress.message}</span>
      <span class="relative tabular-nums">{scanProgress.total > 0 ? `${scanProgress.completed} / ${scanProgress.total} · ${scanPercent}%` : `${scanProgress.completed} 个文件`}</span>
      <div class="absolute inset-x-0 bottom-0 h-0.5 bg-black/30"><div class="h-full bg-accent transition-[width] duration-200" class:animate-pulse={scanProgress.total === 0} style={`width:${scanProgress.total > 0 ? scanPercent : 100}%`}></div></div>
    </div>
  {/if}

  {#if snapshot}
    <section class="grid min-h-0 flex-1" style={`grid-template-columns:${leftWidth}px 5px minmax(0,1fr) 5px ${rightWidth}px`}>
      <aside class="flex min-h-0 flex-col border-r border-border bg-panel">
        <div class="space-y-2 border-b border-border p-2">
          <select bind:value={filter} on:change={updateQuery} class="h-8 w-full rounded border border-border bg-[#111316] px-2 text-xs"><option value="all">全部照片</option><option value="tagged">已标记</option><option value="untagged">未标记</option><option value="complete">配对完整</option><option value="incomplete">配对缺失</option><option value="conflict">配对冲突</option><option value="missing">文件丢失</option></select>
          <div class="flex gap-1"><select bind:value={sortField} on:change={updateQuery} class="h-8 min-w-0 flex-1 rounded border border-border bg-[#111316] px-2 text-xs"><option value="file_name">文件名</option><option value="relative_path">相对路径</option><option value="modified_time">修改时间</option><option value="tagged_status">标注状态</option><option value="missing_status">缺失状态</option></select><button class="h-8 w-9 rounded border border-border bg-[#111316] text-xs" on:click={() => { sortDirection = sortDirection === 'asc' ? 'desc' : 'asc'; updateQuery(); }} aria-label="切换排序方向">{sortDirection === 'asc' ? '↑' : '↓'}</button></div>
          <div class="rounded-lg border border-border bg-black/10 p-1.5"><label class="mb-1 block px-0.5 text-[9px] font-semibold uppercase tracking-widest text-muted" for="smart-kind">智能分类</label><select id="smart-kind" bind:value={smartKind} on:change={changeSmartKind} class="h-8 w-full rounded border border-border bg-[#111316] px-2 text-xs"><option value="none">不筛选</option><option value="date">拍摄日期</option><option value="location">地理坐标</option><option value="similarity">相似照片组</option></select>{#if smartKind !== 'none'}<select bind:value={smartGroup} on:change={changeSmartGroup} class="mt-1 h-8 w-full rounded border border-border bg-[#111316] px-2 text-xs"><option value="">全部有数据的照片</option>{#each smartOptions as option}<option value={option}>{option}</option>{/each}</select>{#if smartOptions.length === 0}<p class="px-1 pt-1.5 text-[10px] text-muted">暂无数据，请重新扫描照片。</p>{/if}{/if}</div>
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-1.5">
          {#each photos as photo, index (photo.id)}
            <button class:active={index === selectedIndex} class="mb-1 flex w-full gap-2 rounded-lg border border-transparent p-1.5 text-left hover:bg-white/5" on:click={() => choosePhoto(index)}>
              <div class="grid h-14 w-16 shrink-0 place-items-center overflow-hidden rounded bg-[#0d0e10] text-[10px] text-muted">{#if photo.thumbnailDataUrl}<img src={photo.thumbnailDataUrl} alt="" class="h-full w-full object-contain" />{:else}{photo.isMissing ? '丢失' : photo.pairStatus === 'conflict' ? '冲突' : 'RAW'}{/if}</div>
              <div class="min-w-0 flex-1 py-0.5"><div class="truncate text-xs font-medium">{photo.displayBaseName}</div><div class="mt-1 truncate text-[10px] text-muted">{photo.relativeDir || '根目录'}</div><div class="mt-1 flex items-center gap-1 text-[9px]">{#if photo.isMissing}<span class="rounded bg-danger/15 px-1.5 py-0.5 text-danger">丢失</span>{:else if photo.pairStatus === 'conflict'}<span class="rounded bg-amber-400/15 px-1.5 py-0.5 text-amber-300">冲突</span>{:else if photo.pairStatus !== 'complete'}<span class="rounded bg-sky-400/15 px-1.5 py-0.5 text-sky-300">缺配对</span>{/if}{#if photo.tagId}<span class="truncate rounded bg-accent/10 px-1.5 py-0.5 text-accent">{photo.tagPath}</span>{/if}</div></div>
            </button>
          {:else}<div class="p-5 text-center text-xs text-muted">当前筛选没有照片</div>{/each}
        </div>
      </aside>
      <button class="cursor-col-resize bg-transparent hover:bg-accent/40" on:pointerdown={(event) => resize('left', event)} aria-label="调整缩略图面板宽度"></button>
      <section class="relative min-w-0 overflow-hidden bg-[#0b0c0e]" on:wheel={zoomPreview}>
        {#if previewData}<div class="grid h-full w-full place-items-center overflow-auto p-8"><img src={previewData} alt={currentPhoto?.displayBaseName} class="max-h-full max-w-full select-none object-contain shadow-2xl transition-transform duration-100" draggable="false" style={`transform:scale(${zoom})`} /></div>
        {:else if previewLoading}<div class="grid h-full place-items-center text-sm text-muted">正在加载 JPEG 预览…</div>
        {:else if currentPhoto}<div class="grid h-full place-items-center"><div class="max-w-sm text-center"><div class="text-5xl opacity-30">◇</div><h2 class="mt-4 text-lg font-medium">{currentPhoto.displayBaseName}</h2><p class="mt-2 text-sm text-muted">{currentPhoto.isMissing ? '源文件已丢失' : currentPhoto.pairStatus === 'conflict' ? '配对存在冲突，请处理后重新扫描' : '缺少 JPEG，ARW 第一版不提供预览'}</p>{#if currentPhoto.isMissing}<Button class="mt-5" variant="danger" size="sm" on:click={deleteCurrentMissing}>删除数据库记录</Button>{/if}</div></div>
        {:else}<div class="grid h-full place-items-center text-sm text-muted">扫描照片目录后，从左侧选择照片</div>{/if}
        {#if previewData}<button class="absolute bottom-4 right-4 rounded-md border border-border bg-black/60 px-3 py-1.5 text-xs text-muted backdrop-blur" on:click={() => zoom = 1}>{Math.round(zoom * 100)}% · 重置</button>{/if}
      </section>
      <button class="cursor-col-resize bg-transparent hover:bg-accent/40" on:pointerdown={(event) => resize('right', event)} aria-label="调整标签面板宽度"></button>
      <aside class="flex min-h-0 flex-col border-l border-border bg-panel">
        <div class="border-b border-border p-3"><div class="text-[10px] font-semibold uppercase tracking-[.18em] text-muted">当前照片标签</div><div class="mt-1 min-h-5 truncate text-sm text-accent">{currentPhoto?.tagPath ?? '未标记'}</div></div>
        <details class="group border-b border-border" open>
          <summary class="flex cursor-pointer list-none items-center justify-between px-3 py-2 text-[10px] font-semibold uppercase tracking-[.18em] text-muted"><span>EXIF 信息</span><span class="transition-transform group-open:rotate-180">⌄</span></summary>
          {#if currentPhoto}
            <div class="grid max-h-52 grid-cols-[76px_minmax(0,1fr)] gap-x-2 gap-y-1 overflow-y-auto px-3 pb-3 text-[10px] leading-5">
              <span class="text-muted">尺寸</span><span>{currentPhoto.pixelWidth && currentPhoto.pixelHeight ? `${currentPhoto.pixelWidth} × ${currentPhoto.pixelHeight}` : '—'}</span>
              <span class="text-muted">拍摄时间</span><span>{currentPhoto.capturedAt ?? '—'}</span>
              <span class="text-muted">相机</span><span class="truncate" title={`${currentPhoto.cameraMake ?? ''} ${currentPhoto.cameraModel ?? ''}`}>{[currentPhoto.cameraMake, currentPhoto.cameraModel].filter(Boolean).join(' ') || '—'}</span>
              <span class="text-muted">镜头</span><span class="truncate" title={currentPhoto.lensModel ?? ''}>{currentPhoto.lensModel ?? '—'}</span>
              <span class="text-muted">曝光</span><span>{currentPhoto.exposureTime ?? '—'}</span>
              <span class="text-muted">光圈</span><span>{currentPhoto.aperture ?? '—'}</span>
              <span class="text-muted">ISO</span><span>{currentPhoto.iso ?? '—'}</span>
              <span class="text-muted">焦距</span><span>{currentPhoto.focalLength ?? '—'}</span>
              <span class="text-muted">坐标</span><span>{currentPhoto.latitude !== null && currentPhoto.longitude !== null ? `${currentPhoto.latitude.toFixed(5)}, ${currentPhoto.longitude.toFixed(5)}` : '—'}</span>
              {#if currentPhoto.similarityGroup}<span class="text-muted">智能组</span><span class="text-accent">{currentPhoto.similarityGroup}</span>{/if}
            </div>
          {:else}<p class="px-3 pb-3 text-[10px] text-muted">选择照片后显示 EXIF。</p>{/if}
        </details>
        <div class="flex flex-wrap gap-1 border-b border-border p-2"><Button size="sm" on:click={() => startTagDialog('root')}>＋根标签</Button><Button size="sm" disabled={!selectedTag} on:click={() => startTagDialog('child')}>＋子标签</Button><Button size="sm" variant="ghost" disabled={!selectedTag} on:click={() => startTagDialog('rename')}>重命名</Button><Button size="sm" variant="ghost" disabled={!selectedTag} on:click={inspectDelete}>删除</Button></div>
        <div class="min-h-0 flex-1 overflow-y-auto py-2">{#each roots as root (root.id)}<TagBranch tag={root} tags={snapshot.tags} selectedId={selectedTagId} assignedId={currentPhoto?.tagId ?? null} onSelect={(id) => assignSelectedTag(id)} />{:else}<div class="p-5 text-center text-xs text-muted">创建第一个层级标签</div>{/each}</div>
        <div class="border-t border-border p-3 text-[10px] leading-5 text-muted">单击立即分配 · 空格重复分配并前进<br />↑ ↓ 切换照片 · ⌘/Ctrl+S 同步</div>
      </aside>
    </section>
    <footer class="flex h-8 shrink-0 items-center gap-4 border-t border-border bg-[#15171a] px-3 text-[11px] text-muted"><span>{photos.length ? selectedIndex + 1 : 0} / {photos.length}</span><span>项目总数 {snapshot.counts.total}</span><span>已标记 {snapshot.counts.tagged}</span><span>缺配对 {snapshot.counts.incomplete}</span><span>冲突 {snapshot.counts.conflicts}</span><span>丢失 {snapshot.counts.missing}</span><span class="ml-auto max-w-[35%] truncate">{snapshot.project.photoRoot}</span></footer>
  {:else}
    <section class="min-h-0 flex-1 overflow-y-auto bg-[radial-gradient(circle_at_top,#20242a_0,#101113_58%)] p-8"><div class="mx-auto flex min-h-full max-w-3xl flex-col justify-center py-8"><div class="text-center"><div class="mx-auto grid h-20 w-20 place-items-center rounded-3xl border border-accent/30 bg-accent/10 text-4xl font-black text-accent">N</div><h1 class="mt-7 text-3xl font-semibold">整理照片，不改动原片</h1><p class="mt-3 leading-7 text-muted">递归配对 JPEG 与 Sony ARW，读取 EXIF 并智能归类，再按标签目录安全导出。</p><div class="mt-7 flex justify-center gap-3"><Button variant="primary" on:click={createProject}>新建项目</Button><Button on:click={openProject}>打开已有项目</Button></div></div>{#if recentProjects.length}<div class="mt-12"><div class="mb-3 flex items-center justify-between"><h2 class="text-sm font-semibold">最近打开</h2><span class="text-[10px] text-muted">最多保留 6 个项目</span></div><div class="grid gap-2 sm:grid-cols-2">{#each recentProjects as recent}<button class="group rounded-xl border border-border bg-panel/80 p-3 text-left transition hover:border-accent/40 hover:bg-white/5" on:click={() => openRecentProject(recent.path)}><div class="flex items-center gap-3"><div class="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-accent/10 text-sm font-bold text-accent">N</div><div class="min-w-0"><div class="truncate text-sm font-medium group-hover:text-accent">{recent.name}</div><div class="mt-0.5 truncate text-[10px] text-muted">{recent.photoRoot}</div></div></div></button>{/each}</div></div>{/if}</div></section>
  {/if}
</main>

{#if tagDialog}<Modal title={tagDialog === 'root' ? '新建根标签' : tagDialog === 'child' ? '新建子标签' : '重命名标签'} on:close={() => tagDialog = null}><form on:submit|preventDefault={submitTag}><label class="mb-1.5 block text-xs text-muted" for="tag-name">标签名称</label><input id="tag-name" bind:value={tagName} use:focusOnMount class="h-10 w-full rounded-md border border-border bg-[#101215] px-3 text-sm outline-none focus:border-accent" placeholder="例如：风景" /><p class="mt-2 text-[11px] text-muted">同级名称必须唯一，不能包含跨平台文件名非法字符。</p><div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => tagDialog = null}>取消</Button><Button type="submit" variant="primary">保存</Button></div></form></Modal>{/if}

{#if deleteDialog}<Modal title="删除标签" description={`将删除“${deleteDialog.tag.name}”及其 ${deleteDialog.impact.tagCount - 1} 个子标签。`} on:close={() => deleteDialog = null}>{#if deleteDialog.impact.affectedPhotos > 0}<p class="rounded-lg border border-danger/30 bg-danger/10 p-3 text-sm">标签子树正在被 <strong>{deleteDialog.impact.affectedPhotos}</strong> 张照片使用。请选择取消，或先将这些照片设为未标记再删除。</p>{:else}<p class="text-sm text-muted">该标签子树没有照片引用，可以安全删除。</p>{/if}<div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => deleteDialog = null}>不删除</Button><Button variant="danger" on:click={confirmDelete}>{deleteDialog.impact.affectedPhotos > 0 ? '取消标记并删除' : '确认删除'}</Button></div></Modal>{/if}

{#if exportDialog}<Modal title="确认批量导出" description={`将导出到 ${exportDialog.root}`} on:close={() => exportDialog = null}><div class="grid grid-cols-2 gap-2 text-sm"><div class="rounded bg-white/5 p-3">可导出 <strong class="float-right text-accent">{exportDialog.plan.eligible}</strong></div><div class="rounded bg-white/5 p-3">同名冲突 <strong class="float-right text-amber-300">{exportDialog.plan.conflicts}</strong></div><div class="rounded bg-white/5 p-3">未标记 <strong class="float-right">{exportDialog.plan.skippedUntagged}</strong></div><div class="rounded bg-white/5 p-3">缺失/不完整 <strong class="float-right">{exportDialog.plan.skippedMissing + exportDialog.plan.skippedIncomplete}</strong></div></div>{#if exportDialog.plan.conflicts > 0}<label class="mt-4 block text-xs text-muted" for="strategy">本次所有同名冲突</label><select id="strategy" bind:value={exportDialog.strategy} class="mt-1 h-10 w-full rounded border border-border bg-[#101215] px-3 text-sm"><option value="skip">全部跳过</option><option value="overwrite">全部覆盖</option><option value="rename">全部自动改名</option></select>{/if}<div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => exportDialog = null}>取消</Button><Button variant="primary" on:click={confirmExport}>开始导出</Button></div></Modal>{/if}

<ToastViewport />

<style>.active { border-color: rgba(217,255,91,.4); background: rgba(217,255,91,.08); }</style>
