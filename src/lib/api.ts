import { invoke } from '@tauri-apps/api/core';
import type {
  BaiduDirectoryListing, BaiduStatus, DeleteImpact, ExportPlan, ExportSummary, PhotoEntry, PhotoQuery, ProjectInfo, ProjectSnapshot, ScanSummary
} from './types';

export const api = {
  createProject: (databasePath: string, photoRoot: string) =>
    invoke<ProjectInfo>('create_project', { databasePath, photoRoot }),
  replaceProject: (databasePath: string, photoRoot: string) =>
    invoke<ProjectInfo>('replace_project', { databasePath, photoRoot }),
  openProject: (databasePath: string) => invoke<ProjectInfo>('open_project', { databasePath }),
  snapshot: (query?: PhotoQuery) => invoke<ProjectSnapshot>('get_snapshot', { query }),
  scan: () => invoke<ScanSummary>('scan_project'),
  photoRootHasChanges: () => invoke<boolean>('photo_root_has_changes'),
  preview: (photoId: number) => invoke<string>('get_photo_preview', { photoId }),
  similarityPhotos: (photoId: number) => invoke<PhotoEntry[]>('get_similarity_photos', { photoId }),
  setSimilarityHidden: (photoId: number, hidden: boolean) => invoke<void>('set_similarity_hidden', { photoId, hidden }),
  createTag: (name: string, parentId: number | null) => invoke<number>('create_tag', { name, parentId }),
  renameTag: (id: number, name: string) => invoke<void>('rename_tag', { id, name }),
  deleteImpact: (id: number) => invoke<DeleteImpact>('inspect_tag_delete', { id }),
  deleteTag: (id: number, unassign: boolean) => invoke<void>('delete_tag', { id, unassign }),
  assignTag: (photoId: number, tagId: number | null) => invoke<void>('assign_tag', { photoId, tagId }),
  assignTags: (photoIds: number[], tagId: number | null) => invoke<number>('assign_tags', { photoIds, tagId }),
  createManualSimilarityGroup: (photoIds: number[]) => invoke<string>('create_manual_similarity_group', { photoIds }),
  removeFromSimilarityGroup: (photoId: number) => invoke<void>('remove_from_similarity_group', { photoId }),
  dissolveSimilarityGroup: (photoId: number) => invoke<number>('dissolve_similarity_group', { photoId }),
  setTagShortcut: (tagId: number, shortcutKey: string | null) => invoke<void>('set_tag_shortcut', { tagId, shortcutKey }),
  setPhotoTrashed: (photoId: number, trashed: boolean) => invoke<void>('set_photo_trashed', { photoId, trashed }),
  saveSort: (field: string, direction: string) => invoke<void>('save_sort', { field, direction }),
  deleteMissing: (ids: number[]) => invoke<number>('delete_missing', { ids }),
  checkpoint: () => invoke<void>('checkpoint'),
  planExport: (exportRoot: string) => invoke<ExportPlan>('plan_export', { exportRoot }),
  exportPhotos: (exportRoot: string, strategy: string) =>
    invoke<ExportSummary>('export_photos', { exportRoot, strategy }),
  baiduStatus: () => invoke<BaiduStatus>('baidu_status'),
  openBaiduAuthorization: () => invoke<string>('open_baidu_authorization'),
  disconnectBaidu: () => invoke<void>('disconnect_baidu'),
  listBaiduFolders: (directory?: string) => invoke<BaiduDirectoryListing>('list_baidu_folders', { directory }),
  createBaiduFolder: (parent: string, name: string) => invoke<string>('create_baidu_folder', { parent, name }),
  planBaiduExport: (root: string) => invoke<ExportPlan>('plan_baidu_export', { root }),
  exportToBaidu: (root: string, strategy: string) => invoke<ExportSummary>('export_to_baidu', { root, strategy })
};
