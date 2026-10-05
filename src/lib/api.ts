import { invoke } from '@tauri-apps/api/core';
import type {
  DeleteImpact, ExportPlan, ExportSummary, PhotoQuery, ProjectInfo, ProjectSnapshot, ScanSummary
} from './types';

export const api = {
  createProject: (databasePath: string, photoRoot: string) =>
    invoke<ProjectInfo>('create_project', { databasePath, photoRoot }),
  replaceProject: (databasePath: string, photoRoot: string) =>
    invoke<ProjectInfo>('replace_project', { databasePath, photoRoot }),
  openProject: (databasePath: string) => invoke<ProjectInfo>('open_project', { databasePath }),
  snapshot: (query?: PhotoQuery) => invoke<ProjectSnapshot>('get_snapshot', { query }),
  scan: () => invoke<ScanSummary>('scan_project'),
  preview: (photoId: number) => invoke<string>('get_photo_preview', { photoId }),
  createTag: (name: string, parentId: number | null) => invoke<number>('create_tag', { name, parentId }),
  renameTag: (id: number, name: string) => invoke<void>('rename_tag', { id, name }),
  deleteImpact: (id: number) => invoke<DeleteImpact>('inspect_tag_delete', { id }),
  deleteTag: (id: number, unassign: boolean) => invoke<void>('delete_tag', { id, unassign }),
  assignTag: (photoId: number, tagId: number | null) => invoke<void>('assign_tag', { photoId, tagId }),
  saveSort: (field: string, direction: string) => invoke<void>('save_sort', { field, direction }),
  deleteMissing: (ids: number[]) => invoke<number>('delete_missing', { ids }),
  checkpoint: () => invoke<void>('checkpoint'),
  setPhotoRoot: (photoRoot: string) => invoke<void>('set_photo_root', { photoRoot }),
  planExport: (exportRoot: string) => invoke<ExportPlan>('plan_export', { exportRoot }),
  exportPhotos: (exportRoot: string, strategy: string) =>
    invoke<ExportSummary>('export_photos', { exportRoot, strategy })
};
