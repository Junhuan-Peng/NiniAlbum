# Nini Albums

JPEG + Sony ARW 层级标签标注与导出工具。应用使用 Tauri v2、Svelte 4、Rust 和 SQLite 构建，不修改源照片；用户可主动选择导出到本地或百度网盘应用目录。

支持单张或批量分配/取消标签、手动组成和修正相似照片组，以及在实际复制前逐项预览导出结果与冲突。

## 开发

环境要求：Node.js 20+、Rust stable，以及当前平台的 Tauri v2 系统依赖。

```bash
npm install
npm run tauri dev
```

百度网盘登录使用应用内的本机临时回调服务。先在百度开放平台登记以下回调地址：

```bash
http://127.0.0.1:8787/oauth/baidu/callback
```

然后复制 `.env.example` 为被 Git 忽略的 `.env.baidu.local`，填入个人应用的 AppKey 和 SecretKey。开发和 release 构建都会自动将这两个值注入桌面应用。

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
npm run tauri build -- --bundles app
```

macOS `.app` 位于 `src-tauri/target/release/bundle/macos/`；需要在对应操作系统上完成打包。

## 项目数据

- 每个项目对应一个 `.sqlite` 文件，并关联一个照片根目录。
- 照片路径以相对于根目录的形式存储，移动根目录后可重新定位。
- SQLite 启用外键和 WAL，标签及扫描操作即时持久化。
- 缩略图写入数据库；ARW 不解码、不生成预览。
- 标签名称可用 `/` 一次创建多级路径；照片可软标记为待删除并在回收站恢复，源文件不会被删除。
- 每个项目可为标签绑定唯一的数字快捷键 `1`–`9`，浏览时按数字即可分配标签并自动跳到下一张。
- 数据库结构版本存放在 `project_settings.schema_version`。后续结构变更应增加版本并执行向前迁移，不能删除旧数据库。
- 个人版在构建期内置百度网盘 AppKey 和 SecretKey；登录时只临时监听本机回调地址，授权完成后立即停止。Access Token 和 Refresh Token 保存在操作系统钥匙串中。导出目标限定为用户选择的空目录。

完整产品和数据规则见 [photo_tagger_vibe_prompt.md](./photo_tagger_vibe_prompt.md)。
