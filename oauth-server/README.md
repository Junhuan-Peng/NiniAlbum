# Nini Albums OAuth Server

该服务负责百度网盘 OAuth 授权、用户绑定和 Token 刷新。照片不会经过该服务；桌面端取得短期 Access Token 后直接上传到百度网盘。

## 身份与数据

- 桌面端首次运行时生成 256 位随机安装凭据并保存到系统钥匙串；请求后端时作为 Bearer Token 使用。
- 后端仅保存安装凭据的 SHA-256 摘要，并将其与百度返回的稳定用户标识 `uk` 绑定。
- 百度 Access Token 和 Refresh Token 使用 AES-256-GCM 加密后写入 SQLite。
- `BAIDU_SECRET_KEY` 和 Token 加密密钥只存在于服务端环境变量。
- 当前身份是“安装实例”。以后增加 Nini 账号时，可将 `device_hash` 关联到账号 ID，从而支持同一用户多设备登录。

## 本地运行

1. 在百度开放平台将授权回调地址配置为：

   `http://127.0.0.1:8787/oauth/baidu/callback`

2. 生成 Token 加密密钥：

   ```bash
   openssl rand -base64 32
   ```

3. 参考 `.env.example` 设置环境变量，然后启动：

   ```bash
   cargo run --manifest-path oauth-server/Cargo.toml
   ```

4. 让桌面端连接本地服务：

   ```bash
   NINI_BAIDU_BACKEND_URL=http://127.0.0.1:8787 npm run tauri dev
   ```

服务提供以下接口：

- `POST /v1/baidu/oauth/start`：创建一次性授权事务并返回百度登录地址；
- `GET /oauth/baidu/callback`：校验 `state`、交换 Token、读取百度 `uk` 并建立绑定；
- `GET /v1/baidu/oauth/status`：查询当前安装实例的登录状态；
- `GET /v1/baidu/token`：返回短期 Access Token，必要时由后端刷新；
- `DELETE /v1/baidu/oauth/binding`：退出并删除该安装实例的百度绑定。

## 生产部署

- 必须为服务配置 HTTPS，并在百度开放平台登记完全一致的 HTTPS 回调地址；
- 使用持久化磁盘保存 `NINI_OAUTH_DATABASE`；
- 通过部署平台的 Secret 管理保存 `BAIDU_SECRET_KEY` 和 `NINI_TOKEN_ENCRYPTION_KEY`；
- 构建桌面应用时设置 `NINI_BAIDU_BACKEND_URL=https://你的授权服务域名`；
- 不要把 `.env`、SQLite 数据库、SecretKey 或加密密钥提交到仓库。
