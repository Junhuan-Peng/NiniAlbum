# Nini Albums

本地 JPEG + Sony ARW 层级标签标注与导出工具。应用使用 Tauri v2、Svelte 4、Rust 和 SQLite 构建，不上传或修改源照片。

## 开发

环境要求：Node.js 20+、Rust stable，以及当前平台的 Tauri v2 系统依赖。

```bash
npm install
npm run tauri dev
```

## 检查与测试

```bash
npm run check
npm run build
cd src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
```

## 构建桌面应用

```bash
npm run tauri build
```

构建产物位于 `src-tauri/target/release/bundle/`。macOS 构建 `.app`/`.dmg`，Windows 构建安装包和可执行文件；需要在对应操作系统上完成打包。

## 项目数据

- 每个项目对应一个 `.sqlite` 文件，并关联一个照片根目录。
- 照片路径以相对于根目录的形式存储，移动根目录后可重新定位。
- SQLite 启用外键和 WAL，标签及扫描操作即时持久化。
- 缩略图写入数据库；ARW 不解码、不生成预览。
- 标签名称可用 `/` 一次创建多级路径；照片可软标记为待删除并在回收站恢复，源文件不会被删除。
- 每个项目可为标签绑定唯一的数字快捷键 `1`–`9`，浏览时按数字即可立即分配标签。
- 数据库结构版本存放在 `project_settings.schema_version`。后续结构变更应增加版本并执行向前迁移，不能删除旧数据库。

完整产品和数据规则见 [photo_tagger_vibe_prompt.md](./photo_tagger_vibe_prompt.md)。
