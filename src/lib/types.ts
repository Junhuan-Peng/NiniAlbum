export interface ProjectInfo {
  databasePath: string;
  photoRoot: string;
  photoRootAvailable: boolean;
  sortField: SortField;
  sortDirection: SortDirection;
}

export type SortField = 'file_name' | 'relative_path' | 'modified_time' | 'tagged_status' | 'missing_status';
export type SortDirection = 'asc' | 'desc';
export type PhotoFilter = 'all' | 'tagged' | 'untagged' | 'complete' | 'incomplete' | 'conflict' | 'missing';

export interface TagNode {
  id: number;
  parentId: number | null;
  name: string;
  sortOrder: number;
  photoCount: number;
}

export interface PhotoEntry {
  id: number;
  relativeDir: string;
  displayBaseName: string;
  pairStatus: 'complete' | 'missing_jpeg' | 'missing_raw' | 'conflict';
  isMissing: boolean;
  tagId: number | null;
  tagPath: string | null;
  jpegRelativePath: string | null;
  rawRelativePath: string | null;
  modifiedTime: number | null;
  thumbnailDataUrl: string | null;
  pixelWidth: number | null;
  pixelHeight: number | null;
  orientation: number | null;
  capturedAt: string | null;
  cameraMake: string | null;
  cameraModel: string | null;
  lensModel: string | null;
  exposureTime: string | null;
  aperture: string | null;
  iso: number | null;
  focalLength: string | null;
  latitude: number | null;
  longitude: number | null;
  dateGroup: string | null;
  locationGroup: string | null;
  similarityGroup: string | null;
}

export interface ProjectCounts { total: number; tagged: number; missing: number; conflicts: number; incomplete: number; }
export interface ProjectSnapshot { project: ProjectInfo; tags: TagNode[]; photos: PhotoEntry[]; counts: ProjectCounts; }
export interface PhotoQuery { filter?: PhotoFilter; sortField?: SortField; sortDirection?: SortDirection; }

export interface ScanSummary {
  discoveredFiles: number; totalGroups: number; complete: number; missingJpeg: number;
  missingRaw: number; conflicts: number; movedFiles: number; warnings: string[];
}

export interface ScanProgress { phase: 'discovering' | 'fingerprinting' | 'thumbnails' | 'database' | 'complete'; completed: number; total: number; message: string; }

export interface DeleteImpact { tagCount: number; affectedPhotos: number; }
export interface ExportPlan { eligible: number; conflicts: number; skippedUntagged: number; skippedIncomplete: number; skippedMissing: number; }
export interface ExportFailure { photoId: number; name: string; reason: string; }
export interface ExportSummary {
  succeeded: number; skipped: number; renamed: number; failed: number; skippedUntagged: number;
  skippedIncomplete: number; skippedMissing: number; failures: ExportFailure[];
}
