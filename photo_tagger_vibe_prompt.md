# 照片层级标签标注工具｜Vibe Coding 完整 Prompt

> 项目：跨平台照片标注 GUI（macOS 为主，兼容 Windows）  
> 技术栈：Tauri v2 + Vite + Svelte 4 + TypeScript + Rust + SQLite + Tailwind CSS + shadcn-svelte v1（Melt UI）  
> 开发模式：由 AI 辅助生成代码。必须遵守本文全部约束，不得擅自替换框架或数据存储方案。

## 1. 项目目标

开发一款完全本地运行的桌面照片标注工具，用于给照片组绑定单个层级标签，并按标签层级目录批量导出原始文件。

- 一个项目只关联一个照片根目录；
- 递归扫描照片根目录及其所有子目录；
- 将同一目录内主文件名相同的 JPEG 与 Sony ARW 组成照片组；
- 标签、照片索引、扫描状态、文件指纹和缩略图均保存在 SQLite 项目文件中；
- 导出时同时复制照片组内的 JPEG 和 ARW，不移动、不转码、不修改原图；
- 单窗口 GUI，无多页面路由，不使用 Web/SSR，不连接云服务。

## 2. 硬性技术约束

1. 使用 **Tauri v2**。Rust 后端负责文件 I/O、SQLite 读写、目录扫描、文件指纹、缩略图生成和导出；Svelte 前端只负责界面、状态展示和交互。
2. 前端使用 **Vite + Svelte 4 + TypeScript**，禁止 Svelte 5 和 Runes 语法。
3. UI 使用 **shadcn-svelte v1 + Melt UI + Tailwind CSS**。组件源码放入项目，不引入重型 UI 框架。
4. 禁止 Web Component、SvelteKit、React 和重型状态管理库；全局状态使用原生 Svelte store。
5. 一个项目对应一个 `.sqlite` 文件，不使用 JSON 保存标签或照片索引。
6. Rust 使用 `rusqlite` 的 `bundled` 特性，不依赖系统 SQLite。
7. 所有路径操作使用 Rust `Path`/`PathBuf` 等平台原生 API，禁止手动拼接 `/` 或 `\`。
8. 仅从 JPEG 生成预览图和缩略图，不解析 ARW 进行预览。
9. 数据修改即时持久化，不维护长事务，不提供撤销或重做。
10. 第一版只识别 `.jpg`、`.jpeg` 和 `.arw`，扩展名匹配忽略大小写。

## 3. 核心领域规则

### 3.1 项目规则

- 一个项目仅关联一个照片根目录；
- 数据库保存照片根目录和文件相对路径；运行时使用原生路径 API 合成完整路径；
- 打开项目后，如果照片根目录不存在，应提示目录不可用，不能静默创建空目录；当前项目不允许重新选择照片根目录，需要用户恢复原路径或新建项目；
- 新建项目遇到已有 SQLite 时，必须让用户选择“打开并升级”“强制覆盖”或取消；升级保留现有数据，覆盖只删除项目数据库及其 WAL/SHM，不删除照片原文件；
- 所有标签分配、标签编辑、项目配置和扫描结果均即时写入 SQLite；
- 标签等业务修改始终即时写入 SQLite；软件设置可启用定时 checkpoint，并配置 1、5、10 或 30 分钟的自动保存间隔；
- 新建项目后自动执行第一次递归扫描；之后每 30 秒轻量检查支持格式文件的路径、大小和修改时间，发现变化时提醒用户点击刷新图标；

### 3.2 照片组与配对规则

照片组的逻辑分组键为：

```text
相对目录 + 大小写不敏感的主文件名
```

- 主文件名匹配时忽略大小写，但保留磁盘上的原始文件名及大小写；
- 正常照片组必须由一个 JPEG 与一个 ARW 一一配对；
- 单独 JPEG 仍建立记录，状态为 `missing_raw`；
- 单独 ARW 仍建立记录，状态为 `missing_jpeg`；
- 同一目录中出现 `A.jpg` 与 `A.jpeg`，或出现多个大小写不同但归一化后同名的 JPEG/ARW 时，状态为 `conflict`；
- 冲突时不得自动猜测或覆盖候选文件；用户需在文件系统中处理后重新扫描；
- 只有恰好包含一个 JPEG 的非冲突照片组才能生成缩略图；
- 仅 ARW 的照片组显示明确的“缺少 JPEG，无法预览”占位状态。
- 缩略图容器按校正方向后的真实宽高比显示，不得将横图和竖图强制装入同一种横向比例；
- 主预览支持滚轮和按钮缩放、放大后拖拽平移、双击或快捷键复位。

### 3.3 层级标签规则

- 一张照片组只能绑定一个标签节点；`NULL` 表示未标记；
- 父节点与叶子节点均可绑定照片；
- 同一父节点下的标签名称大小写不敏感且必须唯一；根节点之间同样唯一；
- 单击标签立即分配给当前照片组；
- 按空格将当前选中的标签再次分配给当前照片组，然后自动跳到下一张；
- 修改标签名称只更新标签表，不批量改写照片记录；
- 删除未被引用的标签时，同时级联删除其子节点，但必须先检查整个子树是否被照片引用；
- 如果待删除节点或其任一子节点已被照片引用，弹窗提供“取消删除”或“将相关照片设为未标记并删除整个标签子树”；
- “取消标记并删除”必须在单个数据库事务内完成。

### 3.4 标签名与导出目录安全

- 创建或重命名标签时禁止空名称和纯空白名称；
- 校验 Windows/macOS 不安全字符，包括 `< > : " / \ | ? *`、控制字符、尾部空格和尾部句点；
- Windows 保留名称（如 `CON`、`PRN`、`AUX`、`NUL`、`COM1`、`LPT1`）不得直接作为标签目录名；
- UI 优先阻止保存非法名称；导出层仍需二次校验并使用确定性的安全替换规则；
- 标签完整路径通过递归 CTE 动态查询，例如 `风景 > 青海 > 茶卡盐湖`。

## 4. SQLite 数据库设计

数据库连接初始化后必须执行：

```sql
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
```

### 4.1 `project_settings`：项目配置

```sql
CREATE TABLE project_settings (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  photo_root TEXT NOT NULL,
  sort_field TEXT NOT NULL DEFAULT 'file_name'
    CHECK (sort_field IN (
      'file_name', 'relative_path', 'modified_time',
      'tagged_status', 'missing_status'
    )),
  sort_direction TEXT NOT NULL DEFAULT 'asc'
    CHECK (sort_direction IN ('asc', 'desc')),
  schema_version INTEGER NOT NULL DEFAULT 3,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

该表只允许一条记录。`photo_root` 是项目唯一关联的照片根目录。

### 4.2 `tag_tree`：层级标签树

```sql
CREATE TABLE tag_tree (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  parent_id INTEGER NULL
    REFERENCES tag_tree(id) ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (length(trim(name)) > 0),
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX idx_tag_sibling_name
  ON tag_tree(parent_id, name COLLATE NOCASE)
  WHERE parent_id IS NOT NULL;

CREATE UNIQUE INDEX idx_tag_root_name
  ON tag_tree(name COLLATE NOCASE)
  WHERE parent_id IS NULL;

CREATE INDEX idx_tag_parent_sort
  ON tag_tree(parent_id, sort_order, name);
```

不能依赖外键级联直接处理已使用标签。删除前必须由业务层递归检查整个子树的引用情况，并按用户选择执行事务。

### 4.3 `scan_runs`：扫描批次

```sql
CREATE TABLE scan_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  started_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  completed_at TEXT NULL,
  status TEXT NOT NULL DEFAULT 'running'
    CHECK (status IN ('running', 'completed', 'failed')),
  error_message TEXT NULL
);
```

只有扫描完整成功后，才能根据 `last_seen_scan_id` 标记丢失文件；扫描中断或失败时不得误标记。

### 4.4 `photo_entries`：照片组

```sql
CREATE TABLE photo_entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  relative_dir TEXT NOT NULL,
  pairing_key TEXT NOT NULL,
  display_base_name TEXT NOT NULL,
  pair_status TEXT NOT NULL DEFAULT 'complete'
    CHECK (pair_status IN (
      'complete', 'missing_jpeg', 'missing_raw', 'conflict'
    )),
  is_missing INTEGER NOT NULL DEFAULT 0
    CHECK (is_missing IN (0, 1)),
  tag_id INTEGER NULL REFERENCES tag_tree(id) ON DELETE RESTRICT,
  thumbnail_blob BLOB NULL,
  thumbnail_mime TEXT NULL,
  pixel_width INTEGER NULL,
  pixel_height INTEGER NULL,
  orientation INTEGER NULL,
  captured_at TEXT NULL,
  camera_make TEXT NULL,
  camera_model TEXT NULL,
  lens_model TEXT NULL,
  exposure_time TEXT NULL,
  aperture TEXT NULL,
  iso INTEGER NULL,
  focal_length TEXT NULL,
  latitude REAL NULL,
  longitude REAL NULL,
  perceptual_hash INTEGER NULL,
  date_group TEXT NULL,
  location_group TEXT NULL,
  similarity_group TEXT NULL,
  is_similarity_hidden INTEGER NOT NULL DEFAULT 0
    CHECK (is_similarity_hidden IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  UNIQUE(relative_dir, pairing_key)
);

CREATE INDEX idx_photo_tag_id ON photo_entries(tag_id);
CREATE INDEX idx_photo_pair_status ON photo_entries(pair_status);
CREATE INDEX idx_photo_missing ON photo_entries(is_missing);
CREATE INDEX idx_photo_sort_name ON photo_entries(display_base_name);
CREATE INDEX idx_photo_sort_path ON photo_entries(relative_dir);
```

- `pairing_key` 是主文件名的大小写不敏感归一化值；
- `display_base_name` 保留用于 UI 展示的原始主文件名；
- `pair_status` 描述配对完整性；
- `is_missing` 表示整个历史照片组在最近一次成功扫描中未找到；
- EXIF 字段来自 JPEG，只读入库，不回写 JPEG 或 ARW；旧项目在打开时自动迁移；
- `date_group` 按拍摄日期生成，`location_group` 将 GPS 坐标按约 1 公里网格归类；
- `perceptual_hash` 用于在拍摄时间相差 10 分钟内识别高度相似照片，命中后写入稳定的 `similarity_group`；
- 相似组是照片浏览的固有上下文，不属于智能筛选项；选择组内照片时在预览区右侧显示完整组；
- 用户可将组内照片从主列表隐藏，状态写入 `is_similarity_hidden`。隐藏照片仍可在相似组侧栏查看和恢复，并且每组至少保留一个未隐藏成员作为入口；
- 配对状态与丢失状态相互独立，例如历史上完整配对的照片组移动失败后可以是 `complete + is_missing=1`。

### 4.5 `photo_files`：照片组内的实际文件

```sql
CREATE TABLE photo_files (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  photo_entry_id INTEGER NOT NULL
    REFERENCES photo_entries(id) ON DELETE CASCADE,
  file_kind TEXT NOT NULL CHECK (file_kind IN ('jpeg', 'arw')),
  original_file_name TEXT NOT NULL,
  relative_path TEXT NOT NULL UNIQUE,
  file_size INTEGER NOT NULL,
  modified_time REAL NOT NULL,
  quick_fingerprint TEXT NOT NULL,
  content_hash TEXT NULL,
  is_missing INTEGER NOT NULL DEFAULT 0
    CHECK (is_missing IN (0, 1)),
  last_seen_scan_id INTEGER NULL REFERENCES scan_runs(id),
  created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_file_entry_kind
  ON photo_files(photo_entry_id, file_kind);
CREATE INDEX idx_file_quick_fingerprint
  ON photo_files(file_size, quick_fingerprint);
CREATE INDEX idx_file_content_hash
  ON photo_files(content_hash);
CREATE INDEX idx_file_last_seen
  ON photo_files(last_seen_scan_id);
```

独立文件表允许保留配对冲突中的多个 JPEG/ARW 候选，不会因为单列路径设计而覆盖冲突文件。

### 4.6 递归查询标签完整路径

```sql
WITH RECURSIVE tag_path(id, parent_id, path) AS (
  SELECT id, parent_id, name
  FROM tag_tree
  WHERE parent_id IS NULL

  UNION ALL

  SELECT child.id,
         child.parent_id,
         tag_path.path || '>' || child.name
  FROM tag_tree AS child
  JOIN tag_path ON child.parent_id = tag_path.id
)
SELECT path FROM tag_path WHERE id = ?1;
```

## 5. 扫描、同步与移动识别

### 5.1 扫描流程

1. 创建状态为 `running` 的扫描批次；
2. 递归遍历照片根目录，避免符号链接循环，收集 JPEG/ARW；
3. 按“相对目录 + 归一化主文件名”分组；
4. 判断每组为完整、缺少 JPEG、缺少 ARW 或冲突；
5. 更新已有记录的路径、文件大小、修改时间和扫描批次；
6. 新文件写入数据库；
7. 通过内容指纹识别移动或重命名的历史文件；
8. 扫描完整成功后，将本批次未发现的文件和照片组标记为丢失；
9. 将扫描批次改为 `completed`。不可恢复错误应标记为 `failed`，不得执行丢失标记阶段。

重新扫描必须保留已有 `tag_id`，不能因为路径、配对或状态变化而清除标签。

### 5.2 内容指纹策略

- 快速指纹由文件大小及文件首尾固定大小数据块计算，建议每端读取 1 MiB；
- 使用稳定哈希算法（如 BLAKE3）；
- 快速指纹只匹配一个历史缺失文件时，可作为移动候选；
- 存在多个候选时必须计算完整内容哈希确认，不能仅凭文件名、大小或修改时间判断；
- 对 JPEG 与 ARW 分别识别移动；配对文件共同移动后更新照片组相对目录和配对状态；
- 完整哈希按需计算并缓存到 `content_hash`，避免每次扫描完整读取大型 ARW；
- 无法唯一确认时保留旧记录为丢失，并把新文件作为新记录，不能错误继承标签。

### 5.3 丢失记录管理

- 支持筛选全部丢失照片组；
- 删除丢失记录只删除数据库索引和缩略图，不执行磁盘删除；
- 支持单条和批量删除；
- 文件重新出现或被识别为移动后，清除丢失标记并保留原标签。

## 6. 缩略图与预览

- 缩略图只从 JPEG 生成并写入 `photo_entries.thumbnail_blob`；
- 生成前降采样，不能一次性长期持有全部原图；
- JPEG 大小或修改时间变化时重新生成缩略图；
- ARW 不解码、不生成预览；
- 中央区域显示当前照片组的 JPEG 降采样图，支持滚轮缩放和居中；
- 缺少 JPEG、文件丢失或配对冲突时显示明确状态，不自动选择冲突候选。

## 7. 排序与筛选

第一版支持以下排序字段及升序/降序：文件名、相对路径、修改时间、标注状态、缺失状态。排序设置保存到 `project_settings`，修改时间排序使用照片组内现存文件的最新修改时间。

至少支持以下筛选：全部、已标记、未标记、配对完整、配对缺失、配对冲突、文件丢失。

## 8. 批量导出

1. 用户选择导出根目录；
2. 只处理 `tag_id IS NOT NULL`、配对完整且文件实际存在的照片组；
3. 未配对、冲突或丢失记录跳过，并分别统计；
4. 使用递归 CTE 获取标签完整路径并创建层级目录；
5. 同时复制 JPEG 与 ARW，不移动、转码或重写文件内容；
6. 标签路径的每一级执行跨平台安全校验；
7. 冲突策略包括跳过、覆盖、自动改名；弹窗支持“应用到本次所有冲突”；
8. 自动改名使用确定性后缀，并确保 JPEG 与 ARW 使用相同的新主文件名；
9. 复制后恢复源文件的文件系统修改时间；创建时间在平台允许时尽可能保留，但不作跨平台硬性保证；
10. JPEG/ARW 内部 EXIF 和拍摄时间随原始字节复制自然保留，不进行元数据改写；
11. 完成后显示成功、跳过、失败、缺失和冲突数量，并可查看失败原因。

## 9. UI 布局与交互

```text
┌────────────┬────────────────────┬──────────────────────┐
│ 缩略图列表 │                    │ 标签树面板           │
│ （左）     │   大图预览区       │ - 层级树选择器       │
│            │   支持滚轮缩放     │ - 新增/重命名/删除   │
│            │                    │ - 当前标签完整路径   │
├────────────┴────────────────────┴──────────────────────┤
│ 状态栏：当前位置 / 总数 / 已标记 / 缺失 / 冲突         │
│ 项目身份｜项目菜单｜刷新图标｜导出图标｜设置图标          │
└───────────────────────────────────────────────────────┘
```

- 三个主面板支持拖拽分割；
- 标签树支持展开、折叠和单选；
- 单击标签立即写入当前照片组；
- 危险操作必须使用确认弹窗；
- 扫描、缩略图生成和导出显示进度，不阻塞 UI 主线程；
- 后台任务错误通过 toast 和可展开详情展示。

## 10. 快捷键

- `↑` / `↓`：上一张或下一张照片组；
- `Space`：重复分配当前选中标签并跳到下一张；
- 设置对话框集中管理软件设置，包括自动保存开关和保存间隔；
- 输入框、弹窗或可编辑控件获得焦点时，不触发照片快捷键。

## 11. 数据一致性与错误处理

- 标签删除、批量取消标记和扫描结果合并使用短事务；
- 所有 SQLite 连接启用外键；
- 异常退出后依赖 SQLite WAL 恢复已提交操作；
- 导出单文件失败不终止整批任务，但记录失败原因；
- 某个子目录无法读取时记录警告并展示；扫描不完整时不执行全局丢失标记；
- 新照片根目录必须先验证可访问性；
- 数据库迁移使用 `schema_version`，禁止以删除旧库的方式升级。

## 12. 第一版明确不做

- 多标签；
- 撤销和重做；
- HEIC、PNG 或除 Sony ARW 之外的 RAW 格式；
- ARW 解码或 RAW 预览；
- 云同步、账号系统和多人协作；
- 自动修改、移动或删除源照片；
- Web、SSR 或移动端版本。

## 13. Rust 依赖参考

```toml
[dependencies]
tauri = { version = "2" }
rusqlite = { version = "0.29", features = ["bundled", "chrono"] }
image = "0.25"
walkdir = "2.5"
blake3 = "1"
filetime = "0.2"
serde = { version = "1", features = ["derive"] }
thiserror = "2"
```

仅在实际需要时增加依赖。异步长任务可使用 Tauri 提供的异步能力或精简的 Tokio 特性，禁止无理由启用 `tokio` 的 `full` 特性。

## 14. 输出与验收要求

1. `src/` 为 Svelte 前端，`src-tauri/` 为 Rust 后端；
2. 代码注释简洁，只说明关键规则和非直观逻辑；
3. 项目可通过标准 Tauri v2 开发命令运行；
4. 支持构建 macOS `.app` 和 Windows 安装包/可执行程序；
5. 不引入与第一版无关的依赖；
6. Rust 单元测试至少覆盖配对、冲突识别、标签路径、非法名称、移动识别和导出改名；
7. 集成测试至少覆盖重新扫描保留标签、失败扫描不误标丢失、删除已使用标签的两种选择；
8. 使用临时目录测试文件操作，不依赖用户真实照片目录；
9. 数据库迁移、事务失败和文件 I/O 错误返回可理解的错误信息；
10. 完成后提供运行、构建、测试和数据库迁移说明。
