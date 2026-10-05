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
  import type { BaiduDirectoryListing, BaiduStatus, DeleteImpact, ExportPlan, ExportProgress, PhotoEntry, PhotoFilter, ProjectSnapshot, ScanProgress, SortDirection, SortField, TagNode } from './lib/types';

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
  let previewRequest = 0;
  let scanProgress: ScanProgress | null = null;
  let zoom = 1;
  let panX = 0;
  let panY = 0;
  let panning = false;
  let leftWidth = 248;
  let rightWidth = 310;
  let tagDialog: 'root' | 'child' | 'rename' | null = null;
  let tagName = '';
  let shortcutDialog: TagNode | null = null;
  let shortcutKey = '';
  let deleteDialog: { tag: TagNode; impact: DeleteImpact } | null = null;
  let exportDestinationDialog = false;
  let exportDialog: { destination: 'local' | 'baidu'; root: string; plan: ExportPlan; strategy: 'skip' | 'overwrite' | 'rename' } | null = null;
  let baiduBrowser: BaiduDirectoryListing | null = null;
  let baiduNewFolder = '';
  let baiduStatus: BaiduStatus = { configured: false, authorized: false, userName: null, error: null };
  let baiduLoginPending = false;
  let baiduLoginAttempt = 0;
  let existingProjectDialog: { databasePath: string; photoRoot: string } | null = null;
  let recentProjects: { path: string; name: string; photoRoot: string; lastOpened: number }[] = [];
  let smartKind: 'none' | 'date' | 'location' = 'none';
  let smartGroup = '';
  let selectedPhotoId: number | null = null;
  let selectedPhotoIds = new Set<number>();
  let selectionAnchorIndex: number | null = null;
  let similarPhotos: PhotoEntry[] = [];
  let similarLoading = false;
  let similarityRequest = 0;
  let projectMenuOpen = false;
  let settingsDialog = false;
  let autoSaveEnabled = true;
  let autoSaveIntervalMinutes = 5;
  let draftAutoSaveEnabled = true;
  let draftAutoSaveIntervalMinutes = 5;
  let lastAutoSave = Date.now();
  let lastChangeCheck = 0;
  let maintenanceRunning = false;
  let sourceChanged = false;

  const previewCache = new Map<number, string>();
  const previewInFlight = new Map<number, Promise<string>>();
  const previewCacheLimit = 16;
  let previewCacheGeneration = 0;
  let previewWarmRequest = 0;
  let previewWarmQueue = Promise.resolve();

  $: sourcePhotos = snapshot?.photos ?? [];
  $: smartOptions = Array.from(new Set(sourcePhotos.map((photo) => smartValue(photo, smartKind)).filter((value): value is string => Boolean(value)))).sort();
  $: classifiedPhotos = smartKind === 'none'
    ? sourcePhotos
    : sourcePhotos.filter((photo) => smartGroup ? smartValue(photo, smartKind) === smartGroup : Boolean(smartValue(photo, smartKind)));
  $: photos = filter === 'trash' ? classifiedPhotos : classifiedPhotos.filter((photo) => !photo.isSimilarityHidden);
  $: currentPhoto = similarPhotos.find((photo) => photo.id === selectedPhotoId)
    ?? sourcePhotos.find((photo) => photo.id === selectedPhotoId)
    ?? photos[selectedIndex]
    ?? null;
  $: currentListIndex = currentPhoto ? photos.findIndex((photo) => photo.id === currentPhoto.id) : -1;
  $: hiddenSimilarityCount = sourcePhotos.filter((photo) => photo.isSimilarityHidden).length;
  $: selectedPhotoCount = selectedPhotoIds.size;
  $: roots = (snapshot?.tags ?? []).filter((tag) => tag.parentId === null);
  $: selectedTag = snapshot?.tags.find((tag) => tag.id === selectedTagId) ?? null;
  $: scanPercent = scanProgress && scanProgress.total > 0
    ? Math.min(100, Math.round((scanProgress.completed / scanProgress.total) * 100))
    : 0;
  $: projectFolderName = snapshot ? pathLastPart(snapshot.project.photoRoot) : 'Nini Albums';
  $: projectInitial = Array.from(projectFolderName.trim())[0]?.toUpperCase() ?? 'N';

  const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error);

  function smartValue(photo: PhotoEntry, kind: typeof smartKind) {
    if (kind === 'date') return photo.dateGroup;
    if (kind === 'location') return photo.locationGroup;
    return null;
  }

  function projectName(path: string) {
    return path.split(/[\\/]/).pop()?.replace(/\.sqlite$/i, '') || '未命名项目';
  }

  function pathLastPart(path: string) {
    return path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() || path;
  }

  function saveSettings() {
    autoSaveEnabled = draftAutoSaveEnabled;
    autoSaveIntervalMinutes = draftAutoSaveIntervalMinutes;
    localStorage.setItem('nini-albums-settings', JSON.stringify({ autoSaveEnabled, autoSaveIntervalMinutes }));
    lastAutoSave = Date.now();
    settingsDialog = false;
    toast('设置已保存。', 'success');
  }

  function openSettings() {
    draftAutoSaveEnabled = autoSaveEnabled;
    draftAutoSaveIntervalMinutes = autoSaveIntervalMinutes;
    settingsDialog = true;
    api.baiduStatus().then((status) => baiduStatus = status).catch(() => undefined);
  }

  function thumbnailStyle(photo: PhotoEntry) {
    const width = photo.pixelWidth ?? 4;
    const height = photo.pixelHeight ?? 3;
    const ratio = Math.max(0.45, Math.min(2.2, width / height));
    const longEdge = 76;
    const shortEdge = Math.max(42, Math.round(longEdge / Math.max(ratio, 1 / ratio)));
    return ratio >= 1
      ? `width:${longEdge}px;height:${shortEdge}px`
      : `width:${shortEdge}px;height:${longEdge}px`;
  }

  function rememberProject(path: string, photoRoot: string) {
    const item = { path, name: projectName(path), photoRoot, lastOpened: Date.now() };
    recentProjects = [item, ...recentProjects.filter((recent) => recent.path !== path)].slice(0, 6);
    localStorage.setItem('nini-albums-recent-projects', JSON.stringify(recentProjects));
  }

  function canPreview(photo: PhotoEntry | null | undefined): photo is PhotoEntry {
    return Boolean(photo?.jpegRelativePath && !photo.isMissing && photo.pairStatus !== 'conflict');
  }

  function clearPreviewCache() {
    previewCacheGeneration += 1;
    previewWarmRequest += 1;
    previewRequest += 1;
    previewCache.clear();
    previewInFlight.clear();
    previewData = null;
    previewLoading = false;
    resetPreview();
  }

  function cachedPreview(photoId: number) {
    const data = previewCache.get(photoId);
    if (!data) return null;
    previewCache.delete(photoId);
    previewCache.set(photoId, data);
    return data;
  }

  function rememberPreview(photoId: number, data: string) {
    previewCache.delete(photoId);
    previewCache.set(photoId, data);
    while (previewCache.size > previewCacheLimit) {
      const oldest = previewCache.keys().next().value;
      if (oldest === undefined) break;
      previewCache.delete(oldest);
    }
  }

  async function decodePreview(data: string) {
    const image = new Image();
    image.src = data;
    try { await image.decode(); }
    catch { /* WebKit 仍可在后续 img 元素中解码，不阻断预览。 */ }
  }

  function fetchPreview(photo: PhotoEntry) {
    const cached = cachedPreview(photo.id);
    if (cached) return Promise.resolve(cached);
    const existing = previewInFlight.get(photo.id);
    if (existing) return existing;

    const generation = previewCacheGeneration;
    const request = api.preview(photo.id).then(async (data) => {
      await decodePreview(data);
      if (generation === previewCacheGeneration) rememberPreview(photo.id, data);
      return data;
    });
    previewInFlight.set(photo.id, request);
    void request.then(() => {
      if (previewInFlight.get(photo.id) === request) previewInFlight.delete(photo.id);
    }, () => {
      if (previewInFlight.get(photo.id) === request) previewInFlight.delete(photo.id);
    });
    return request;
  }

  function prewarmNearbyPreviews(photoId: number) {
    const candidates: PhotoEntry[] = [];
    const seen = new Set<number>([photoId]);
    const listIndex = photos.findIndex((photo) => photo.id === photoId);
    if (listIndex >= 0) {
      for (const offset of [-1, 1, -2, 2]) {
        const photo = photos[listIndex + offset];
        if (canPreview(photo) && !seen.has(photo.id)) { seen.add(photo.id); candidates.push(photo); }
      }
    }
    if (similarPhotos.some((photo) => photo.id === photoId)) {
      for (const photo of similarPhotos) {
        if (canPreview(photo) && !seen.has(photo.id)) { seen.add(photo.id); candidates.push(photo); }
        if (candidates.length >= 4) break;
      }
    }
    const request = ++previewWarmRequest;
    previewWarmQueue = previewWarmQueue.catch(() => {}).then(async () => {
      for (const photo of candidates.slice(0, 4)) {
        if (request !== previewWarmRequest) return;
        try { await fetchPreview(photo); }
        catch { /* 预热失败不打扰当前照片。 */ }
      }
    });
  }

  async function changeSmartKind() {
    clearPhotoSelection();
    smartGroup = '';
    selectedIndex = 0;
    await tick();
    selectedPhotoId = photos[0]?.id ?? null;
    await Promise.all([loadSimilarityPhotos(selectedPhotoId), loadPreview(currentPhoto)]);
  }

  async function changeSmartGroup() {
    clearPhotoSelection();
    selectedIndex = 0;
    await tick();
    selectedPhotoId = photos[0]?.id ?? null;
    await Promise.all([loadSimilarityPhotos(selectedPhotoId), loadPreview(currentPhoto)]);
  }

  async function runBusy<T>(label: string, task: () => Promise<T>): Promise<T | undefined> {
    busy = true; busyLabel = label;
    try { return await task(); }
    catch (error) { toast(errorMessage(error), 'error', 6000); }
    finally { busy = false; busyLabel = ''; }
  }

  async function refresh(preserveId: number | null = currentPhoto?.id ?? null) {
    const result = await api.snapshot({ filter, sortField, sortDirection });
    snapshot = result;
    sortField = result.project.sortField;
    sortDirection = result.project.sortDirection;
    await tick();
    const preservable = preserveId && sourcePhotos.some((photo) => photo.id === preserveId) ? preserveId : null;
    const next = preservable ? photos.findIndex((photo) => photo.id === preservable) : 0;
    selectedIndex = next >= 0 ? next : Math.min(selectedIndex, Math.max(0, photos.length - 1));
    selectedPhotoId = preservable ?? photos[selectedIndex]?.id ?? null;
    await tick();
    await Promise.all([loadSimilarityPhotos(selectedPhotoId), loadPreview(currentPhoto)]);
  }

  async function createProject() {
    const photoRoot = await open({ directory: true, multiple: false, title: '选择照片根目录' });
    if (!photoRoot || Array.isArray(photoRoot)) return;
    let databasePath = await save({ title: '保存项目数据库', defaultPath: 'nini-albums.sqlite', filters: [{ name: 'SQLite 项目', extensions: ['sqlite'] }] });
    if (!databasePath) return;
    if (!databasePath.toLowerCase().endsWith('.sqlite')) databasePath += '.sqlite';
    let created = false;
    busy = true; busyLabel = '正在创建项目…';
    try {
      await api.createProject(databasePath, photoRoot);
      clearPreviewCache();
      filter = 'all'; selectedIndex = 0; await refresh(null);
      rememberProject(databasePath, photoRoot);
      toast('项目已创建，正在首次扫描照片。', 'success');
      created = true;
    } catch (error) {
      const message = errorMessage(error);
      if (message.includes('项目数据库已存在')) existingProjectDialog = { databasePath, photoRoot };
      else toast(message, 'error', 6000);
    } finally { busy = false; busyLabel = ''; }
    if (created) await scanPhotos();
  }

  async function resolveExistingProject(action: 'upgrade' | 'overwrite') {
    if (!existingProjectDialog) return;
    const { databasePath, photoRoot } = existingProjectDialog;
    existingProjectDialog = null;
    const shouldScan = await runBusy(action === 'upgrade' ? '正在升级项目…' : '正在覆盖项目…', async () => {
      const info = action === 'upgrade'
        ? await api.openProject(databasePath)
        : await api.replaceProject(databasePath, photoRoot);
      clearPreviewCache();
      filter = 'all'; smartKind = 'none'; smartGroup = ''; sortField = info.sortField; sortDirection = info.sortDirection; selectedIndex = 0;
      await refresh(null);
      sourceChanged = false; lastChangeCheck = 0;
      rememberProject(databasePath, info.photoRoot);
      toast(action === 'upgrade' ? '旧项目已升级并打开，原有标签和索引已保留。' : '旧项目已覆盖，照片原文件未被修改。', 'success', 6000);
      return action === 'overwrite';
    });
    if (shouldScan) await scanPhotos();
  }

  async function openProject() {
    const path = await open({ multiple: false, directory: false, title: '打开项目数据库', filters: [{ name: 'SQLite 项目', extensions: ['sqlite'] }] });
    if (!path || Array.isArray(path)) return;
    await runBusy('正在打开项目…', async () => {
      const info = await api.openProject(path);
      clearPreviewCache();
      filter = 'all'; sortField = info.sortField; sortDirection = info.sortDirection; selectedIndex = 0;
      await refresh(null);
      sourceChanged = false; lastChangeCheck = 0;
      rememberProject(path, info.photoRoot);
      toast(info.photoRootAvailable ? '项目已打开。' : '项目已打开，但原照片目录不可用，请重新定位。', info.photoRootAvailable ? 'success' : 'error', 6000);
    });
  }

  async function openRecentProject(path: string) {
    await runBusy('正在打开项目…', async () => {
      const info = await api.openProject(path);
      clearPreviewCache();
      filter = 'all'; smartKind = 'none'; smartGroup = ''; sortField = info.sortField; sortDirection = info.sortDirection; selectedIndex = 0;
      await refresh(null);
      sourceChanged = false; lastChangeCheck = 0;
      rememberProject(path, info.photoRoot);
      toast(info.photoRootAvailable ? '项目已打开。' : '原照片目录不可用，请重新定位。', info.photoRootAvailable ? 'success' : 'error');
    });
  }

  async function scanPhotos() {
    scanProgress = { phase: 'discovering', completed: 0, total: 0, message: '正在递归发现照片文件' };
    try {
      await runBusy('正在扫描…', async () => {
        const result = await api.scan(); clearPreviewCache(); sourceChanged = false; await refresh();
        toast(`扫描完成：${result.totalGroups} 组，完整 ${result.complete}，缺配对 ${result.missingJpeg + result.missingRaw}，冲突 ${result.conflicts}`, result.warnings.length ? 'default' : 'success', 6000);
      });
    } finally {
      scanProgress = null;
    }
  }

  async function loadPreview(photo: PhotoEntry | null) {
    const request = ++previewRequest;
    resetPreview();
    if (!canPreview(photo)) { previewData = null; previewLoading = false; return; }
    const requested = photo.id;
    const cached = cachedPreview(requested);
    if (cached) {
      previewData = cached;
      previewLoading = false;
      prewarmNearbyPreviews(requested);
      return;
    }
    previewData = null;
    previewLoading = true;
    try {
      const data = await fetchPreview(photo);
      if (request === previewRequest && currentPhoto?.id === requested) {
        previewData = data;
        prewarmNearbyPreviews(requested);
      }
    }
    catch (error) { toast(`预览加载失败：${errorMessage(error)}`, 'error'); }
    finally { if (request === previewRequest) previewLoading = false; }
  }

  async function choosePhoto(index: number) {
    if (index < 0 || index >= photos.length) return;
    selectedIndex = index; selectedPhotoId = photos[index].id;
    await tick();
    await Promise.all([loadSimilarityPhotos(selectedPhotoId), loadPreview(currentPhoto)]);
  }

  function setPhotoSelection(ids: Iterable<number>) {
    selectedPhotoIds = new Set(ids);
  }

  function togglePhotoSelection(index: number, range = false) {
    if (index < 0 || index >= photos.length) return;
    const next = new Set(selectedPhotoIds);
    if (range && selectionAnchorIndex !== null) {
      const [start, end] = [selectionAnchorIndex, index].sort((a, b) => a - b);
      for (let cursor = start; cursor <= end; cursor += 1) next.add(photos[cursor].id);
    } else if (next.has(photos[index].id)) next.delete(photos[index].id);
    else next.add(photos[index].id);
    selectionAnchorIndex = index;
    selectedPhotoIds = next;
  }

  async function handlePhotoClick(index: number, event: MouseEvent) {
    if (event.shiftKey || event.metaKey || event.ctrlKey) {
      togglePhotoSelection(index, event.shiftKey);
    } else {
      selectionAnchorIndex = index;
    }
    await choosePhoto(index);
  }

  function selectAllPhotos() {
    setPhotoSelection(photos.map((photo) => photo.id));
  }

  function clearPhotoSelection() {
    setPhotoSelection([]);
    selectionAnchorIndex = null;
  }

  async function loadSimilarityPhotos(photoId: number | null) {
    const request = ++similarityRequest;
    if (photoId === null) { similarPhotos = []; return; }
    similarLoading = true;
    try {
      const result = await api.similarityPhotos(photoId);
      if (request === similarityRequest) similarPhotos = result;
    } catch (error) {
      if (request === similarityRequest) similarPhotos = [];
      toast(`相似照片加载失败：${errorMessage(error)}`, 'error');
    } finally {
      if (request === similarityRequest) similarLoading = false;
    }
  }

  async function chooseSimilarPhoto(photo: PhotoEntry) {
    selectedPhotoId = photo.id;
    const listIndex = photos.findIndex((item) => item.id === photo.id);
    if (listIndex >= 0) selectedIndex = listIndex;
    await tick();
    await loadPreview(currentPhoto);
  }

  async function toggleSimilarityHidden(photo: PhotoEntry) {
    const hidden = !photo.isSimilarityHidden;
    if (hidden && similarPhotos.filter((item) => !item.isSimilarityHidden).length <= 1) {
      toast('每个相似组至少保留一张照片在主列表中，方便之后重新打开该组。');
      return;
    }
    try {
      await api.setSimilarityHidden(photo.id, hidden);
      const update = (item: PhotoEntry) => item.id === photo.id ? { ...item, isSimilarityHidden: hidden } : item;
      similarPhotos = similarPhotos.map(update);
      if (snapshot) snapshot = { ...snapshot, photos: snapshot.photos.map(update) };
      await tick();
      const listIndex = photos.findIndex((item) => item.id === selectedPhotoId);
      if (listIndex >= 0) selectedIndex = listIndex;
      toast(hidden ? `已从主列表隐藏 ${photo.displayBaseName}` : `已恢复 ${photo.displayBaseName} 到主列表`, 'success');
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function assignSelectedTag(tagId: number, advance = false) {
    if (!currentPhoto) return;
    selectedTagId = tagId;
    const photoId = currentPhoto.id;
    const targets = selectedPhotoIds.size ? [...selectedPhotoIds] : [photoId];
    const batch = targets.length > 1;
    const nextPhotoId = currentListIndex >= 0
      ? photos[currentListIndex + 1]?.id ?? (filter === 'untagged' ? photos[currentListIndex - 1]?.id ?? photoId : photoId)
      : photoId;
    try {
      await api.assignTags(targets, tagId);
      if (selectedPhotoIds.size) clearPhotoSelection();
      await refresh(advance && !batch ? nextPhotoId : photoId);
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function clearAssignedTags() {
    if (!currentPhoto) return;
    const targets = selectedPhotoIds.size ? [...selectedPhotoIds] : [currentPhoto.id];
    const preserveId = currentPhoto.id;
    try {
      await api.assignTags(targets, null);
      clearPhotoSelection();
      await refresh(preserveId);
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function createManualSimilarityGroup() {
    const ids = [...selectedPhotoIds];
    if (ids.length < 2) { toast('请先批量选择至少两张照片。'); return; }
    try {
      await api.createManualSimilarityGroup(ids);
      const preserveId = currentPhoto?.id ?? ids[0];
      clearPhotoSelection();
      await refresh(preserveId);
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function removeCurrentFromSimilarityGroup() {
    if (!currentPhoto?.similarityGroup) return;
    try {
      const photoId = currentPhoto.id;
      await api.removeFromSimilarityGroup(photoId);
      await refresh(photoId);
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function dissolveCurrentSimilarityGroup() {
    if (!currentPhoto?.similarityGroup || !confirm(`解散“${currentPhoto.similarityGroup}”？组内照片将不再自动重新合并。`)) return;
    try {
      const photoId = currentPhoto.id;
      await api.dissolveSimilarityGroup(photoId);
      await refresh(photoId);
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  function startTagDialog(mode: 'root' | 'child' | 'rename') {
    if ((mode === 'child' || mode === 'rename') && !selectedTag) { toast('请先选择一个标签。'); return; }
    tagName = mode === 'rename' ? selectedTag?.name ?? '' : ''; tagDialog = mode;
  }

  async function submitTag() {
    if (!tagDialog || !tagName.trim()) return;
    try {
      const mode = tagDialog;
      if (mode === 'rename' && selectedTag) await api.renameTag(selectedTag.id, tagName);
      else await api.createTag(tagName, mode === 'child' ? selectedTagId : null);
      tagDialog = null;
      await refresh();
      if (mode === 'rename') toast('标签已重命名。', 'success');
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  function openShortcutDialog() {
    if (!selectedTag) { toast('请先选择一个标签。'); return; }
    shortcutDialog = selectedTag;
    shortcutKey = selectedTag.shortcutKey ?? '';
  }

  function shortcutOwner(key: string) {
    return snapshot?.tags.find((tag) => tag.shortcutKey === key && tag.id !== shortcutDialog?.id) ?? null;
  }

  async function saveTagShortcut() {
    if (!shortcutDialog) return;
    try {
      await api.setTagShortcut(shortcutDialog.id, shortcutKey || null);
      const photoId = currentPhoto?.id ?? null;
      shortcutDialog = null;
      await refresh(photoId);
    } catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function toggleTrashView() {
    clearPhotoSelection();
    filter = filter === 'trash' ? 'all' : 'trash';
    smartKind = 'none'; smartGroup = ''; selectedIndex = 0;
    await runBusy(filter === 'trash' ? '正在打开回收站…' : '正在返回照片列表…', async () => refresh(null));
  }

  async function toggleCurrentTrashed() {
    if (!currentPhoto) return;
    const photo = currentPhoto;
    const trashed = !photo.isTrashed;
    const nextId = photos[selectedIndex + 1]?.id ?? photos[selectedIndex - 1]?.id ?? null;
    try {
      await api.setPhotoTrashed(photo.id, trashed);
      await refresh(nextId);
      toast(trashed ? `已将 ${photo.displayBaseName} 移入回收站。` : `已恢复 ${photo.displayBaseName}。`, 'success');
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
    clearPhotoSelection();
    if (filter === 'trash') { smartKind = 'none'; smartGroup = ''; }
    await runBusy('正在更新列表…', async () => { await api.saveSort(sortField, sortDirection); await refresh(); });
  }

  async function deleteCurrentMissing() {
    if (!currentPhoto?.isMissing || !confirm(`仅删除“${currentPhoto.displayBaseName}”的数据库记录？不会删除磁盘文件。`)) return;
    try { await api.deleteMissing([currentPhoto.id]); await refresh(); toast('丢失记录已删除。', 'success'); }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  async function prepareExport() {
    try { baiduStatus = await api.baiduStatus(); } catch { /* 本地导出仍然可用。 */ }
    exportDestinationDialog = true;
  }

  async function prepareLocalExport() {
    const root = await open({ directory: true, multiple: false, title: '选择导出根目录' });
    if (!root || Array.isArray(root)) return;
    await runBusy('正在检查导出内容…', async () => {
      const plan = await api.planExport(root);
      exportDestinationDialog = false;
      exportDialog = { destination: 'local', root, plan, strategy: plan.conflicts ? 'skip' : 'overwrite' };
    });
  }

  async function openBaiduBrowser(directory?: string) {
    await runBusy('正在读取百度网盘目录…', async () => {
      baiduBrowser = await api.listBaiduFolders(directory);
      exportDestinationDialog = false;
    });
  }

  async function createBaiduExportFolder() {
    if (!baiduBrowser || !baiduNewFolder.trim()) return;
    await runBusy('正在创建百度网盘目录…', async () => {
      const root = await api.createBaiduFolder(baiduBrowser!.currentPath, baiduNewFolder);
      baiduNewFolder = '';
      const plan = await api.planBaiduExport(root);
      baiduBrowser = null;
      exportDialog = { destination: 'baidu', root, plan, strategy: plan.conflicts ? 'skip' : 'overwrite' };
    });
  }

  async function selectBaiduExportRoot() {
    if (!baiduBrowser?.isEmpty) { toast('请选择一个空目录。'); return; }
    await runBusy('正在检查导出内容…', async () => {
      const root = baiduBrowser!.currentPath;
      const plan = await api.planBaiduExport(root);
      baiduBrowser = null;
      exportDialog = { destination: 'baidu', root, plan, strategy: plan.conflicts ? 'skip' : 'overwrite' };
    });
  }

  function baiduParent(path: string) {
    if (path === '/apps/尼尼相册') return null;
    return path.slice(0, path.lastIndexOf('/')) || '/apps/尼尼相册';
  }

  function openBaiduParent() {
    if (!baiduBrowser) return;
    const parent = baiduParent(baiduBrowser.currentPath);
    if (parent) openBaiduBrowser(parent);
  }

  async function confirmExport() {
    if (!exportDialog) return;
    const { root, strategy, destination } = exportDialog; exportDialog = null;
    await runBusy(destination === 'baidu' ? '正在上传到百度网盘…' : '正在复制 JPEG 与 ARW…', async () => {
      const result = destination === 'baidu' ? await api.exportToBaidu(root, strategy) : await api.exportPhotos(root, strategy);
      toast(`导出完成：成功 ${result.succeeded}，跳过 ${result.skipped}，失败 ${result.failed}${result.renamed ? `，改名 ${result.renamed}` : ''}`, result.failed ? 'error' : 'success', 7000);
    });
  }

  async function startBaiduAuthorization() {
    const attempt = ++baiduLoginAttempt;
    baiduLoginPending = true;
    try {
      await api.openBaiduAuthorization();
      toast('已打开百度登录页面，正在等待授权…');
      for (let index = 0; index < 150 && baiduLoginAttempt === attempt; index += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, 2000));
        try {
          const status = await api.baiduStatus();
          baiduStatus = status;
          if (status.error) {
            toast(`百度网盘登录失败：${status.error}`, 'error', 7000);
            return;
          }
          if (status.authorized) {
            toast(`百度网盘登录成功${status.userName ? `：${status.userName}` : ''}`, 'success');
            return;
          }
        } catch { /* 本地回调尚未完成时继续等待。 */ }
      }
      if (baiduLoginAttempt === attempt) toast('登录等待超时，请重新点击登录。', 'error');
    } catch (error) { toast(errorMessage(error), 'error'); }
    finally { if (baiduLoginAttempt === attempt) baiduLoginPending = false; }
  }

  async function disconnectBaidu() {
    baiduLoginAttempt += 1;
    baiduLoginPending = false;
    try { await api.disconnectBaidu(); baiduStatus = await api.baiduStatus(); toast('已移除百度网盘授权。'); }
    catch (error) { toast(errorMessage(error), 'error'); }
  }

  function handleKeydown(event: KeyboardEvent) {
    const target = event.target as HTMLElement;
    if (['INPUT','TEXTAREA','SELECT'].includes(target.tagName) || target.isContentEditable) return;
    if (!snapshot || tagDialog || shortcutDialog || deleteDialog || exportDestinationDialog || exportDialog || baiduBrowser || existingProjectDialog || settingsDialog) return;
    if (previewData && (event.key === '+' || event.key === '=')) { event.preventDefault(); zoomBy(1.2); return; }
    if (previewData && event.key === '-') { event.preventDefault(); zoomBy(1 / 1.2); return; }
    if (previewData && event.key === '0') { event.preventDefault(); resetPreview(); return; }
    if (!event.repeat && !event.metaKey && !event.ctrlKey && !event.altKey && /^[1-9]$/.test(event.key)) {
      const tag = snapshot.tags.find((item) => item.shortcutKey === event.key);
      if (tag && currentPhoto) { event.preventDefault(); void assignSelectedTag(tag.id, true); return; }
    }
    const deletePressed = event.key === 'Delete' || event.key === 'Backspace'
      || event.code === 'Delete' || event.code === 'Backspace';
    if (currentPhoto && deletePressed) { event.preventDefault(); event.stopPropagation(); void toggleCurrentTrashed(); return; }
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
    event.preventDefault(); zoomBy(event.deltaY < 0 ? 1.12 : .89);
  }

  function zoomBy(factor: number) {
    zoom = Math.max(0.2, Math.min(8, zoom * factor));
    if (zoom <= 1) { panX = 0; panY = 0; }
  }

  function resetPreview() {
    zoom = 1; panX = 0; panY = 0; panning = false;
  }

  function startPan(event: PointerEvent) {
    if (!previewData || zoom <= 1 || event.button !== 0) return;
    event.preventDefault();
    panning = true;
    const startX = event.clientX; const startY = event.clientY;
    const initialX = panX; const initialY = panY;
    const target = event.currentTarget as HTMLElement;
    target.setPointerCapture(event.pointerId);
    const move = (next: PointerEvent) => {
      panX = initialX + next.clientX - startX;
      panY = initialY + next.clientY - startY;
    };
    const stop = () => {
      panning = false;
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', stop);
      target.removeEventListener('pointercancel', stop);
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', stop);
    target.addEventListener('pointercancel', stop);
  }

  function focusOnMount(node: HTMLElement) {
    queueMicrotask(() => node.focus());
  }

  async function runBackgroundMaintenance() {
    if (!snapshot || busy || maintenanceRunning) return;
    maintenanceRunning = true;
    const now = Date.now();
    try {
      if (autoSaveEnabled && now - lastAutoSave >= autoSaveIntervalMinutes * 60_000) {
        await api.checkpoint();
        lastAutoSave = now;
      }
      if (now - lastChangeCheck >= 30_000) {
        lastChangeCheck = now;
        const changed = await api.photoRootHasChanges();
        if (changed && !sourceChanged) toast('检测到照片目录发生变化，请点击顶部刷新按钮更新项目。', 'default', 7000);
        sourceChanged = changed;
      }
    } catch { /* 后台检查失败不打断当前操作。 */ }
    finally { maintenanceRunning = false; }
  }

  onMount(() => {
    try {
      const stored = JSON.parse(localStorage.getItem('nini-albums-recent-projects') ?? '[]');
      if (Array.isArray(stored)) recentProjects = stored.slice(0, 6);
    } catch { recentProjects = []; }
    try {
      const stored = JSON.parse(localStorage.getItem('nini-albums-settings') ?? '{}');
      if (typeof stored.autoSaveEnabled === 'boolean') autoSaveEnabled = stored.autoSaveEnabled;
      if ([1, 5, 10, 30].includes(stored.autoSaveIntervalMinutes)) autoSaveIntervalMinutes = stored.autoSaveIntervalMinutes;
    } catch { /* 使用默认设置。 */ }
    const maintenanceTimer = window.setInterval(runBackgroundMaintenance, 15_000);
    let unlisten: (() => void) | undefined;
    let unlistenExport: (() => void) | undefined;
    listen<ScanProgress>('scan-progress', ({ payload }) => {
      if (scanProgress?.phase === payload.phase && scanProgress.message === payload.message
        && payload.completed < scanProgress.completed) return;
      scanProgress = payload;
      busyLabel = payload.total > 0
        ? `${payload.message} ${payload.completed}/${payload.total}`
        : payload.message;
    }).then((dispose) => { unlisten = dispose; }).catch(() => { /* Browser-only preview has no Tauri event bridge. */ });
    listen<ExportProgress>('export-progress', ({ payload }) => {
      busyLabel = payload.totalFiles > 0
        ? `正在上传 ${payload.currentFile} ${payload.completedFiles}/${payload.totalFiles}`
        : '正在准备上传…';
    }).then((dispose) => { unlistenExport = dispose; }).catch(() => { /* Browser-only preview has no Tauri event bridge. */ });
    return () => { unlisten?.(); unlistenExport?.(); window.clearInterval(maintenanceTimer); };
  });
</script>

<svelte:window on:keydown|capture={handleKeydown} on:contextmenu|preventDefault />

<main class="flex h-full min-h-0 flex-col bg-background text-foreground">
  <header class="flex h-16 shrink-0 items-center gap-3 border-b border-border bg-[#15171a] px-4">
    <div class="flex min-w-0 items-center gap-3"><div class="grid h-10 w-10 shrink-0 place-items-center rounded-xl bg-accent text-lg font-black text-black">{projectInitial}</div><div class="min-w-0 leading-tight"><strong class="block max-w-64 truncate text-sm">{projectFolderName}</strong><span class="block max-w-72 truncate text-[10px] text-muted">{snapshot?.project.photoRoot ?? '本地照片标注工具'}</span></div></div>
    {#if snapshot}
      <div class="relative ml-1">
        <button class="grid h-9 w-9 place-items-center rounded-lg border border-border bg-[#22252a] text-lg text-muted hover:border-[#444952] hover:text-foreground" aria-label="项目菜单" aria-expanded={projectMenuOpen} on:click={() => projectMenuOpen = !projectMenuOpen}>•••</button>
        {#if projectMenuOpen}<div class="absolute left-0 top-11 z-40 w-40 rounded-lg border border-border bg-[#1b1e22] p-1.5 text-xs shadow-2xl"><button class="w-full rounded px-3 py-2 text-left hover:bg-white/5" on:click={() => { projectMenuOpen = false; createProject(); }}>新建项目</button><button class="w-full rounded px-3 py-2 text-left hover:bg-white/5" on:click={() => { projectMenuOpen = false; openProject(); }}>打开其他项目</button></div>{/if}
      </div>
      <div class="ml-1 h-6 w-px bg-border"></div>
      <button class="relative grid h-9 w-9 place-items-center rounded-lg border border-border bg-[#22252a] text-muted hover:border-accent/40 hover:text-foreground disabled:opacity-40" disabled={busy} on:click={scanPhotos} aria-label="刷新照片目录" title={sourceChanged ? '检测到文件变化，点击刷新' : '刷新照片目录'}><svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current" stroke-width="2"><path d="M20 11a8 8 0 0 0-14.9-4M4 4v5h5M4 13a8 8 0 0 0 14.9 4M20 20v-5h-5" /></svg>{#if sourceChanged}<span class="absolute -right-0.5 -top-0.5 h-2.5 w-2.5 rounded-full border-2 border-[#15171a] bg-amber-400"></span>{/if}</button>
      <button class="grid h-9 w-9 place-items-center rounded-lg border border-border bg-accent text-black hover:bg-[#e5ff83] disabled:opacity-40" disabled={busy} on:click={prepareExport} aria-label="批量导出" title="批量导出"><svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current" stroke-width="2"><path d="M12 3v12m0-12 4 4m-4-4L8 7M5 14v6h14v-6" /></svg></button>
      <button class:border-danger={filter === 'trash'} class:text-danger={filter === 'trash'} class="flex h-9 items-center gap-2 rounded-lg border border-border bg-[#22252a] px-3 text-xs text-muted hover:border-danger/60 hover:text-danger disabled:opacity-40" disabled={busy} on:click={toggleTrashView} aria-label={filter === 'trash' ? '返回照片列表' : '打开回收站'} title={filter === 'trash' ? '返回照片列表' : '打开回收站'}><svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current" stroke-width="2"><path d="M4 7h16M9 7V4h6v3m3 0-1 13H7L6 7m4 4v5m4-5v5" /></svg><span>{filter === 'trash' ? '返回照片' : '回收站'}</span>{#if snapshot.counts.trashed > 0}<span class="min-w-4 rounded-full bg-danger px-1 text-center text-[8px] font-bold leading-4 text-white">{snapshot.counts.trashed}</span>{/if}</button>
    {/if}
    <button class="ml-auto grid h-9 w-9 shrink-0 place-items-center rounded-lg border border-border bg-[#22252a] text-muted hover:border-[#444952] hover:text-foreground" on:click={openSettings} aria-label="设置" title="设置"><svg viewBox="0 0 24 24" class="h-4 w-4 fill-none stroke-current" stroke-width="2"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6a1.7 1.7 0 0 0 1-1.6v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z"/></svg></button>
    <div class="flex shrink-0 items-center gap-2 text-xs text-muted">{#if busy}<span class="inline-block h-3 w-3 animate-spin rounded-full border-2 border-accent border-r-transparent"></span><span class="max-w-48 truncate">{busyLabel}</span>{/if}</div>
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
          <select bind:value={filter} on:change={updateQuery} class="h-8 w-full rounded border border-border bg-[#111316] px-2 text-xs"><option value="all">全部照片</option><option value="tagged">已标记</option><option value="untagged">未标记</option><option value="complete">配对完整</option><option value="incomplete">配对缺失</option><option value="conflict">配对冲突</option><option value="missing">文件丢失</option><option value="trash">回收站</option></select>
          <div class="flex gap-1"><select bind:value={sortField} on:change={updateQuery} class="h-8 min-w-0 flex-1 rounded border border-border bg-[#111316] px-2 text-xs"><option value="file_name">文件名</option><option value="relative_path">相对路径</option><option value="modified_time">修改时间</option><option value="tagged_status">标注状态</option><option value="missing_status">缺失状态</option></select><button class="h-8 w-9 rounded border border-border bg-[#111316] text-xs" on:click={() => { sortDirection = sortDirection === 'asc' ? 'desc' : 'asc'; updateQuery(); }} aria-label="切换排序方向">{sortDirection === 'asc' ? '↑' : '↓'}</button></div>
          <div class="rounded-lg border border-border bg-black/10 p-1.5"><label class="mb-1 block px-0.5 text-[9px] font-semibold uppercase tracking-widest text-muted" for="smart-kind">智能分类</label><select id="smart-kind" bind:value={smartKind} on:change={changeSmartKind} class="h-8 w-full rounded border border-border bg-[#111316] px-2 text-xs"><option value="none">不筛选</option><option value="date">拍摄日期</option><option value="location">地理坐标</option></select>{#if smartKind !== 'none'}<select bind:value={smartGroup} on:change={changeSmartGroup} class="mt-1 h-8 w-full rounded border border-border bg-[#111316] px-2 text-xs"><option value="">全部有数据的照片</option>{#each smartOptions as option}<option value={option}>{option}</option>{/each}</select>{#if smartOptions.length === 0}<p class="px-1 pt-1.5 text-[10px] text-muted">暂无数据，请重新扫描照片。</p>{/if}{/if}</div>
        </div>
        <div class="flex min-h-10 items-center gap-1 border-b border-border px-2 py-1.5 text-[10px]">
          <button class="rounded border border-border px-2 py-1.5 text-muted hover:border-accent/40 hover:text-foreground" on:click={selectAllPhotos}>全选</button>
          {#if selectedPhotoCount > 0}
            <span class="ml-1 whitespace-nowrap text-accent">已选 {selectedPhotoCount} 张</span>
            <button class="ml-auto rounded border border-border px-2 py-1.5 text-muted hover:border-accent/40 hover:text-foreground disabled:opacity-40" disabled={selectedPhotoCount < 2} on:click={createManualSimilarityGroup}>组成相似组</button>
            <button class="rounded px-2 py-1.5 text-muted hover:bg-white/5 hover:text-foreground" on:click={clearPhotoSelection}>清空</button>
          {:else}
            <span class="ml-1 text-muted">⌘/Ctrl 单选 · Shift 连选</span>
          {/if}
        </div>
        <div class="min-h-0 flex-1 overflow-y-auto p-1.5">
          {#each photos as photo, index (photo.id)}
            <div class:active={photo.id === currentPhoto?.id} class:batch-selected={selectedPhotoIds.has(photo.id)} class="mb-1 flex min-h-[88px] w-full items-center rounded-lg border border-transparent hover:bg-white/5">
              <button class="ml-1.5 grid h-5 w-5 shrink-0 place-items-center rounded border text-[11px] font-bold" class:border-accent={selectedPhotoIds.has(photo.id)} class:bg-accent={selectedPhotoIds.has(photo.id)} class:text-black={selectedPhotoIds.has(photo.id)} class:border-border={!selectedPhotoIds.has(photo.id)} class:text-muted={!selectedPhotoIds.has(photo.id)} aria-label={`${selectedPhotoIds.has(photo.id) ? '取消选择' : '选择'} ${photo.displayBaseName}`} on:click|stopPropagation={() => { togglePhotoSelection(index); choosePhoto(index); }}>{selectedPhotoIds.has(photo.id) ? '✓' : ''}</button>
              <button class="flex min-w-0 flex-1 items-center gap-3 p-2 text-left" on:click={(event) => handlePhotoClick(index, event)}>
                <div class="grid shrink-0 place-items-center overflow-hidden rounded-md bg-[#0d0e10] text-[10px] text-muted shadow-sm" style={thumbnailStyle(photo)}>{#if photo.thumbnailDataUrl}<img src={photo.thumbnailDataUrl} alt="" class="h-full w-full object-cover" />{:else}{photo.isMissing ? '丢失' : photo.pairStatus === 'conflict' ? '冲突' : 'RAW'}{/if}</div>
                <div class="min-w-0 flex-1 py-0.5"><div class="truncate text-xs font-medium">{photo.displayBaseName}</div><div class="mt-1 truncate text-[10px] text-muted">{photo.relativeDir || '根目录'}</div><div class="mt-1 flex items-center gap-1 text-[9px]">{#if photo.isTrashed}<span class="rounded bg-danger/15 px-1.5 py-0.5 text-danger">待删除</span>{:else if photo.isMissing}<span class="rounded bg-danger/15 px-1.5 py-0.5 text-danger">丢失</span>{:else if photo.pairStatus === 'conflict'}<span class="rounded bg-amber-400/15 px-1.5 py-0.5 text-amber-300">冲突</span>{:else if photo.pairStatus !== 'complete'}<span class="rounded bg-sky-400/15 px-1.5 py-0.5 text-sky-300">缺配对</span>{/if}{#if photo.tagId}<span class="truncate rounded bg-accent/10 px-1.5 py-0.5 text-accent">{photo.tagPath}</span>{/if}</div></div>
              </button>
            </div>
          {:else}<div class="p-5 text-center text-xs text-muted">当前筛选没有照片</div>{/each}
        </div>
      </aside>
      <button class="cursor-col-resize bg-transparent hover:bg-accent/40" on:pointerdown={(event) => resize('left', event)} aria-label="调整缩略图面板宽度"></button>
      <section class="flex min-w-0 overflow-hidden bg-[#0b0c0e]">
        <div class="relative min-w-0 flex-1 overflow-hidden" on:wheel={zoomPreview}>
          {#if previewData}<div role="application" aria-label="图片预览：滚轮或按钮缩放，放大后拖拽，双击复位" class:cursor-grabbing={panning} class:cursor-grab={!panning} class="absolute inset-0 flex touch-none items-center justify-center overflow-hidden p-8" on:pointerdown={startPan} on:dblclick={resetPreview}><img src={previewData} alt={currentPhoto?.displayBaseName} class="block max-h-full max-w-full select-none object-contain shadow-2xl" draggable="false" style={`transform:translate(${panX}px,${panY}px) scale(${zoom});transform-origin:center`} /></div>
          {:else if previewLoading}<div class="grid h-full place-items-center text-sm text-muted">正在加载 JPEG 预览…</div>
          {:else if currentPhoto}<div class="grid h-full place-items-center"><div class="max-w-sm text-center"><div class="text-5xl opacity-30">◇</div><h2 class="mt-4 text-lg font-medium">{currentPhoto.displayBaseName}</h2><p class="mt-2 text-sm text-muted">{currentPhoto.isMissing ? '源文件已丢失' : currentPhoto.pairStatus === 'conflict' ? '配对存在冲突，请处理后重新扫描' : '缺少 JPEG，ARW 第一版不提供预览'}</p>{#if currentPhoto.isMissing}<Button class="mt-5" variant="danger" size="sm" on:click={deleteCurrentMissing}>删除数据库记录</Button>{/if}</div></div>
          {:else}<div class="grid h-full place-items-center text-sm text-muted">扫描照片目录后，从左侧选择照片</div>{/if}
          {#if previewData}<div class="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-lg border border-border bg-black/70 p-1 text-xs shadow-xl backdrop-blur"><button class="grid h-8 w-8 place-items-center rounded hover:bg-white/10" on:click={() => zoomBy(1 / 1.2)} aria-label="缩小">−</button><button class="min-w-[76px] rounded px-2 py-2 text-muted hover:bg-white/10 hover:text-foreground" on:click={resetPreview} title="双击图片或按 0 也可复位">{Math.round(zoom * 100)}% · 适合</button><button class="grid h-8 w-8 place-items-center rounded hover:bg-white/10" on:click={() => zoomBy(1.2)} aria-label="放大">＋</button><span class="hidden border-l border-border px-2 text-[10px] text-muted xl:inline">滚轮缩放 · 放大后拖拽</span></div>{/if}
        </div>
        {#if similarLoading || similarPhotos.length > 1}
          <aside class="flex w-44 shrink-0 flex-col border-l border-border bg-[#121417]">
            <div class="border-b border-border px-3 py-2"><div class="text-[10px] font-semibold uppercase tracking-[.16em] text-muted">相似照片</div><div class="mt-0.5 text-xs">{similarPhotos.length} 张{#if similarPhotos.some((photo) => photo.isSimilarityHidden)} · {similarPhotos.filter((photo) => photo.isSimilarityHidden).length} 张已隐藏{/if}</div><div class="mt-2 grid grid-cols-2 gap-1"><button class="rounded border border-border px-1 py-1 text-[9px] text-muted hover:border-accent/40 hover:text-foreground" on:click={removeCurrentFromSimilarityGroup}>移出当前</button><button class="rounded border border-border px-1 py-1 text-[9px] text-muted hover:border-danger/50 hover:text-danger" on:click={dissolveCurrentSimilarityGroup}>解散整组</button></div></div>
            <div class="min-h-0 flex-1 space-y-2 overflow-y-auto p-2">
              {#if similarLoading}<div class="py-6 text-center text-[10px] text-muted">正在加载…</div>{/if}
              {#each similarPhotos as photo (photo.id)}
                <div class:opacity-50={photo.isSimilarityHidden} class:active={photo.id === currentPhoto?.id} class="group rounded-lg border border-transparent p-1.5 hover:bg-white/5">
                  <button class="block w-full text-left" on:click={() => chooseSimilarPhoto(photo)} title={photo.displayBaseName}><div class="grid h-20 w-full place-items-center overflow-hidden rounded bg-black/40">{#if photo.thumbnailDataUrl}<img src={photo.thumbnailDataUrl} alt={photo.displayBaseName} class="h-full w-full object-contain" />{:else}<span class="text-[10px] text-muted">无缩略图</span>{/if}</div><div class="mt-1.5 truncate text-[10px]">{photo.displayBaseName}</div></button>
                  <button class="mt-1 w-full rounded border border-border px-1.5 py-1 text-[9px] text-muted hover:border-accent/40 hover:text-foreground" on:click={() => toggleSimilarityHidden(photo)}>{photo.isSimilarityHidden ? '恢复到主列表' : '从主列表隐藏'}</button>
                </div>
              {/each}
            </div>
          </aside>
        {/if}
      </section>
      <button class="cursor-col-resize bg-transparent hover:bg-accent/40" on:pointerdown={(event) => resize('right', event)} aria-label="调整标签面板宽度"></button>
      <aside class="flex min-h-0 flex-col border-l border-border bg-panel">
        <div class="border-b border-border p-3"><div class="flex items-center justify-between gap-2"><div class="text-[10px] font-semibold uppercase tracking-[.18em] text-muted">当前照片标签</div>{#if currentPhoto}<button class="rounded border border-border px-2 py-1 text-[9px] text-muted hover:border-danger/60 hover:text-danger" on:click={toggleCurrentTrashed} title="快捷键：Delete / Backspace">{currentPhoto.isTrashed ? '恢复照片' : '标记删除'}</button>{/if}</div><div class="mt-1 flex min-h-5 items-center gap-2 truncate text-sm text-accent"><span class="truncate">{currentPhoto?.tagPath ?? '未标记'}</span>{#if currentPhoto?.isTrashed}<span class="shrink-0 rounded bg-danger/15 px-1.5 py-0.5 text-[9px] text-danger">回收站</span>{/if}</div></div>
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
              {#if currentPhoto.similarityGroup}<span class="text-muted">相似组</span><span class="text-accent">{currentPhoto.similarityGroup}</span>{/if}
            </div>
          {:else}<p class="px-3 pb-3 text-[10px] text-muted">选择照片后显示 EXIF。</p>{/if}
        </details>
        <div class="flex flex-wrap gap-1 border-b border-border p-2"><Button size="sm" on:click={() => startTagDialog('root')}>＋根标签</Button><Button size="sm" disabled={!selectedTag} on:click={() => startTagDialog('child')}>＋子标签</Button><Button size="sm" variant="ghost" disabled={!selectedTag} on:click={() => startTagDialog('rename')}>重命名</Button><Button size="sm" variant="ghost" disabled={!selectedTag} on:click={openShortcutDialog}>快捷键</Button><Button size="sm" variant="ghost" disabled={!selectedTag} on:click={inspectDelete}>删除</Button><Button size="sm" variant="ghost" disabled={!currentPhoto || (selectedPhotoCount === 0 && !currentPhoto.tagId)} on:click={clearAssignedTags}>取消标签{selectedPhotoCount ? ` (${selectedPhotoCount})` : ''}</Button></div>
        <div class="min-h-0 flex-1 overflow-y-auto py-2">{#each roots as root (root.id)}<TagBranch tag={root} tags={snapshot.tags} selectedId={selectedTagId} assignedId={currentPhoto?.tagId ?? null} onSelect={(id) => assignSelectedTag(id)} />{:else}<div class="p-5 text-center text-xs text-muted">创建第一个层级标签</div>{/each}</div>
        <div class="border-t border-border p-3 text-[10px] leading-5 text-muted">单击立即分配 · 1–9 分配并前进<br />空格重复分配并前进 · ↑ ↓ 切换照片</div>
      </aside>
    </section>
    <footer class="flex h-8 shrink-0 items-center gap-4 border-t border-border bg-[#15171a] px-3 text-[11px] text-muted"><span>{currentListIndex >= 0 ? currentListIndex + 1 : '组内'} / {photos.length}</span><span>项目总数 {snapshot.counts.total}</span><span>已标记 {snapshot.counts.tagged}</span>{#if hiddenSimilarityCount}<span>相似图隐藏 {hiddenSimilarityCount}</span>{/if}<span>缺配对 {snapshot.counts.incomplete}</span><span>冲突 {snapshot.counts.conflicts}</span><span>丢失 {snapshot.counts.missing}</span>{#if snapshot.counts.trashed}<span class="text-danger">回收站 {snapshot.counts.trashed}</span>{/if}<span class="ml-auto max-w-[35%] truncate">{snapshot.project.photoRoot}</span></footer>
  {:else}
    <section class="min-h-0 flex-1 overflow-y-auto bg-[radial-gradient(circle_at_top,#20242a_0,#101113_58%)] p-8"><div class="mx-auto flex min-h-full max-w-3xl flex-col justify-center py-8"><div class="text-center"><div class="mx-auto grid h-20 w-20 place-items-center rounded-3xl border border-accent/30 bg-accent/10 text-4xl font-black text-accent">N</div><h1 class="mt-7 text-3xl font-semibold">把照片，整理成你熟悉的样子</h1><p class="mt-3 leading-7 text-muted">在本地完成配对、归类与导出，原片始终留在原处。</p><div class="mt-7 flex justify-center gap-3"><Button variant="primary" on:click={createProject}>新建项目</Button><Button on:click={openProject}>打开已有项目</Button></div></div>{#if recentProjects.length}<div class="mt-12"><div class="mb-3 flex items-center justify-between"><h2 class="text-sm font-semibold">最近打开</h2><span class="text-[10px] text-muted">最多保留 6 个项目</span></div><div class="grid gap-2 sm:grid-cols-2">{#each recentProjects as recent}<button class="group rounded-xl border border-border bg-panel/80 p-3 text-left transition hover:border-accent/40 hover:bg-white/5" on:click={() => openRecentProject(recent.path)}><div class="flex items-center gap-3"><div class="grid h-9 w-9 shrink-0 place-items-center rounded-lg bg-accent/10 text-sm font-bold text-accent">N</div><div class="min-w-0"><div class="truncate text-sm font-medium group-hover:text-accent">{recent.name}</div><div class="mt-0.5 truncate text-[10px] text-muted">{recent.photoRoot}</div></div></div></button>{/each}</div></div>{/if}</div></section>
  {/if}
</main>

{#if tagDialog}<Modal title={tagDialog === 'root' ? '新建根标签' : tagDialog === 'child' ? '新建子标签' : '重命名标签'} on:close={() => tagDialog = null}><form on:submit|preventDefault={submitTag}><label class="mb-1.5 block text-xs text-muted" for="tag-name">{tagDialog === 'rename' ? '标签名称' : '标签名称或路径'}</label><input id="tag-name" bind:value={tagName} use:focusOnMount class="h-10 w-full rounded-md border border-border bg-[#101215] px-3 text-sm outline-none focus:border-accent" placeholder={tagDialog === 'rename' ? '例如：风景' : '例如：青海/沿途'} /><p class="mt-2 text-[11px] text-muted">{tagDialog === 'rename' ? '同级名称必须唯一，不能包含跨平台文件名非法字符。' : '使用 / 可一次创建多级标签；已有中间层会自动复用。'}</p><div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => tagDialog = null}>取消</Button><Button type="submit" variant="primary">保存</Button></div></form></Modal>{/if}

{#if shortcutDialog}<Modal title="标签快捷键" description={`为“${shortcutDialog.name}”绑定数字键`} on:close={() => shortcutDialog = null}><form on:submit|preventDefault={saveTagShortcut}><label class="mb-1.5 block text-xs text-muted" for="tag-shortcut">快捷键</label><select id="tag-shortcut" bind:value={shortcutKey} use:focusOnMount class="h-10 w-full rounded-md border border-border bg-[#101215] px-3 text-sm outline-none focus:border-accent"><option value="">不绑定</option>{#each ['1','2','3','4','5','6','7','8','9'] as key}{@const owner = shortcutOwner(key)}<option value={key} disabled={Boolean(owner)}>{key}{owner ? ` · 已绑定 ${owner.name}` : ''}</option>{/each}</select><p class="mt-2 text-[11px] leading-5 text-muted">数字 1–9 在项目内唯一。浏览照片时按下数字会立即分配标签，并自动跳到下一张。</p><div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => shortcutDialog = null}>取消</Button><Button type="submit" variant="primary">保存</Button></div></form></Modal>{/if}

{#if deleteDialog}<Modal title="删除标签" description={`将删除“${deleteDialog.tag.name}”及其 ${deleteDialog.impact.tagCount - 1} 个子标签。`} on:close={() => deleteDialog = null}>{#if deleteDialog.impact.affectedPhotos > 0}<p class="rounded-lg border border-danger/30 bg-danger/10 p-3 text-sm">标签子树正在被 <strong>{deleteDialog.impact.affectedPhotos}</strong> 张照片使用。请选择取消，或先将这些照片设为未标记再删除。</p>{:else}<p class="text-sm text-muted">该标签子树没有照片引用，可以安全删除。</p>{/if}<div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => deleteDialog = null}>不删除</Button><Button variant="danger" on:click={confirmDelete}>{deleteDialog.impact.affectedPhotos > 0 ? '取消标记并删除' : '确认删除'}</Button></div></Modal>{/if}

{#if exportDestinationDialog}<Modal title="导出照片" description="选择导出位置；两种方式都要求使用空目录。" on:close={() => exportDestinationDialog = false}><div class="grid gap-3 sm:grid-cols-2"><button class="rounded-xl border border-border bg-white/5 p-4 text-left hover:border-accent/50" on:click={prepareLocalExport}><strong class="text-sm">导出到本地</strong><p class="mt-1 text-xs leading-5 text-muted">选择一个已存在的空目录，保留 JPEG/ARW 修改时间。</p></button><button class="rounded-xl border border-border bg-white/5 p-4 text-left hover:border-accent/50 disabled:cursor-not-allowed disabled:opacity-45" disabled={!baiduStatus.authorized} on:click={() => openBaiduBrowser()}><strong class="text-sm">导出到百度网盘</strong><p class="mt-1 text-xs leading-5 text-muted">在“我的应用数据/尼尼相册”中选择或新建空目录。</p></button></div>{#if !baiduStatus.authorized}<p class="mt-3 rounded-lg border border-border p-3 text-xs text-muted">百度网盘尚未登录，请先在设置中完成登录授权。</p>{/if}</Modal>{/if}

{#if baiduBrowser}<Modal title="选择百度网盘空目录" description={baiduBrowser.currentPath} on:close={() => baiduBrowser = null}><div class="mb-3 flex gap-2">{#if baiduParent(baiduBrowser.currentPath)}<Button size="sm" variant="ghost" on:click={openBaiduParent}>返回上级</Button>{/if}<Button size="sm" variant="primary" disabled={!baiduBrowser.isEmpty} on:click={selectBaiduExportRoot}>选择当前空目录</Button></div><div class="max-h-56 space-y-1 overflow-y-auto rounded-lg border border-border p-2">{#each baiduBrowser.folders as folder}<button class="flex w-full items-center gap-2 rounded px-3 py-2 text-left text-sm hover:bg-white/5" on:click={() => openBaiduBrowser(folder.path)}><span>▸</span><span class="truncate">{folder.name}</span></button>{:else}<p class="p-3 text-center text-xs text-muted">当前目录没有子目录</p>{/each}</div><div class="mt-4 flex gap-2"><input bind:value={baiduNewFolder} class="h-10 min-w-0 flex-1 rounded-md border border-border bg-[#101215] px-3 text-sm" placeholder="新建空目录名称" /><Button on:click={createBaiduExportFolder} disabled={!baiduNewFolder.trim()}>新建并选择</Button></div>{#if !baiduBrowser.isEmpty}<p class="mt-2 text-xs text-amber-300">当前目录不是空目录，不能作为导出根目录。</p>{/if}</Modal>{/if}

{#if exportDialog}<Modal title={exportDialog.destination === 'baidu' ? '确认导出到百度网盘' : '确认本地导出'} description={`将导出到空目录 ${exportDialog.root}`} on:close={() => exportDialog = null}><div class="grid grid-cols-2 gap-2 text-sm"><div class="rounded bg-white/5 p-3">可导出 <strong class="float-right text-accent">{exportDialog.plan.eligible}</strong></div><div class="rounded bg-white/5 p-3">未标记（根目录） <strong class="float-right">{exportDialog.plan.untagged}</strong></div><div class="rounded bg-white/5 p-3">同名冲突 <strong class="float-right text-amber-300">{exportDialog.plan.conflicts}</strong></div><div class="rounded bg-white/5 p-3">缺失/不完整 <strong class="float-right">{exportDialog.plan.skippedMissing + exportDialog.plan.skippedIncomplete}</strong></div><div class="rounded bg-white/5 p-3">已隐藏 <strong class="float-right">{exportDialog.plan.skippedHidden}</strong></div><div class="rounded bg-white/5 p-3">回收站 <strong class="float-right">{exportDialog.plan.skippedTrashed}</strong></div></div>{#if exportDialog.plan.items.length}<div class="mt-4 flex items-center justify-between"><span class="text-xs font-medium">照片明细</span><span class="text-[10px] text-muted">共 {exportDialog.plan.items.length} 张</span></div><div class="mt-1 max-h-64 overflow-y-auto rounded-lg border border-border bg-black/10"><div class="sticky top-0 grid grid-cols-[minmax(0,1fr)_74px] border-b border-border bg-[#181b1f] px-3 py-2 text-[9px] uppercase tracking-wider text-muted"><span>照片 / 目标</span><span>处理结果</span></div>{#each exportDialog.plan.items as item (item.photoId)}<div class="grid grid-cols-[minmax(0,1fr)_74px] items-center gap-2 border-b border-border/60 px-3 py-2 last:border-b-0"><div class="min-w-0"><div class="truncate text-xs" title={item.name}>{item.name}</div><div class="mt-0.5 truncate text-[9px] text-muted" title={item.detail}>{item.detail}</div></div><span class:text-accent={item.status === 'ready' || item.status === 'untagged'} class:text-amber-300={item.status === 'conflict'} class:text-danger={item.status === 'missing'} class="text-[10px]">{item.status === 'ready' ? '可导出' : item.status === 'conflict' ? '同名冲突' : item.status === 'untagged' ? '导出到根目录' : item.status === 'incomplete' ? '配对不完整' : item.status === 'missing' ? '文件丢失' : '回收站'}</span></div>{/each}</div>{/if}{#if exportDialog.plan.conflicts > 0}<label class="mt-4 block text-xs text-muted" for="strategy">本次所有同名冲突</label><select id="strategy" bind:value={exportDialog.strategy} class="mt-1 h-10 w-full rounded border border-border bg-[#101215] px-3 text-sm"><option value="skip">全部跳过</option><option value="overwrite">全部覆盖</option><option value="rename">全部自动改名</option></select>{/if}<div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => exportDialog = null}>取消</Button><Button variant="primary" on:click={confirmExport}>开始导出</Button></div></Modal>{/if}

{#if existingProjectDialog}<Modal title="项目文件已存在" description={existingProjectDialog.databasePath} on:close={() => existingProjectDialog = null}><div class="space-y-3 text-sm"><div class="rounded-lg border border-border bg-white/5 p-3"><strong class="block">打开并升级</strong><p class="mt-1 text-xs leading-5 text-muted">保留已有标签、扫描记录和项目设置，并自动升级数据库结构。</p></div><div class="rounded-lg border border-danger/30 bg-danger/10 p-3"><strong class="block text-danger">强制覆盖</strong><p class="mt-1 text-xs leading-5 text-muted">删除原项目数据库及其中的标签和索引，再创建空项目；照片目录中的原文件不会被删除。</p></div></div><div class="mt-5 flex flex-wrap justify-end gap-2"><Button variant="ghost" on:click={() => existingProjectDialog = null}>取消</Button><Button on:click={() => resolveExistingProject('upgrade')}>打开并升级</Button><Button variant="danger" on:click={() => resolveExistingProject('overwrite')}>强制覆盖</Button></div></Modal>{/if}

{#if settingsDialog}<Modal title="设置" description="Nini Albums 软件设置" on:close={() => settingsDialog = false}><div class="max-h-[65vh] space-y-4 overflow-y-auto pr-1"><label class="flex cursor-pointer items-start justify-between gap-4 rounded-lg border border-border bg-white/5 p-3"><div><strong class="block text-sm">自动保存</strong><p class="mt-1 text-xs leading-5 text-muted">标签和隐藏状态始终即时入库。开启后还会定期将 SQLite WAL 完整合并到项目文件。</p></div><input type="checkbox" bind:checked={draftAutoSaveEnabled} class="mt-1 h-4 w-4 accent-[#d9ff5b]" /></label><label class="block"><span class="mb-1.5 block text-xs text-muted">自动保存间隔</span><select bind:value={draftAutoSaveIntervalMinutes} disabled={!draftAutoSaveEnabled} class="h-10 w-full rounded-md border border-border bg-[#101215] px-3 text-sm disabled:opacity-45"><option value={1}>每 1 分钟</option><option value={5}>每 5 分钟</option><option value={10}>每 10 分钟</option><option value={30}>每 30 分钟</option></select></label><div class="rounded-lg border border-border p-3 text-xs leading-5 text-muted"><div class="flex justify-between"><span>照片变更检测</span><span class="text-foreground">每 30 秒</span></div><p class="mt-1">只检查支持格式的路径、文件大小和修改时间；发现变化后会提示使用顶部刷新按钮。</p></div><section class="rounded-xl border border-border bg-white/[.03] p-3"><div class="flex items-center justify-between"><strong class="text-sm">百度网盘</strong><span class={baiduStatus.authorized ? 'text-xs text-accent' : 'text-xs text-muted'}>{baiduStatus.authorized ? '已登录' : baiduStatus.configured ? '未登录' : '未配置凭据'}</span></div><p class="mt-1 text-xs leading-5 text-muted">个人版使用应用内置凭据。登录时临时启动本机回调服务，Access Token 和 Refresh Token 仅保存在系统钥匙串中，照片由本机直接上传。</p>{#if !baiduStatus.authorized}<p class="mt-2 text-[10px] leading-5 text-muted">百度开放平台回调地址需设置为 http://127.0.0.1:8787/oauth/baidu/callback</p>{/if}{#if baiduStatus.error}<p class="mt-2 rounded-lg border border-danger/30 bg-danger/10 p-2 text-xs text-danger">{baiduStatus.error}</p>{/if}{#if baiduStatus.authorized}<div class="mt-3 rounded-lg border border-accent/20 bg-accent/5 p-3 text-xs"><span class="text-muted">当前账号</span><strong class="ml-2">{baiduStatus.userName ?? '百度网盘用户'}</strong></div><div class="mt-3"><Button size="sm" variant="ghost" on:click={disconnectBaidu}>退出百度网盘</Button></div>{:else}<div class="mt-3"><Button size="sm" variant="primary" disabled={baiduLoginPending || !baiduStatus.configured} on:click={startBaiduAuthorization}>{baiduLoginPending ? '等待浏览器授权…' : '登录百度网盘'}</Button></div>{/if}</section></div><div class="mt-5 flex justify-end gap-2"><Button variant="ghost" on:click={() => settingsDialog = false}>取消</Button><Button variant="primary" on:click={saveSettings}>保存设置</Button></div></Modal>{/if}

<ToastViewport />

<style>.active { border-color: rgba(217,255,91,.4); background: rgba(217,255,91,.08); }.batch-selected { box-shadow: inset 0 0 0 1px rgba(100,180,255,.55); background: rgba(100,180,255,.07); }</style>
