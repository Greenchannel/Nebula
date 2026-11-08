# Nebula（星云）架构规范

> 零服务器、纯局域网的 P2P 即时通讯 + 去中心化动态平台。
> GUI（Tauri v2）/ TUI（ratatui）/ Web（同源单页）共享同一套内核与协议。

## 1. Workspace

```
Cargo.toml                     # workspace: core + tui（app 为独立 Tauri workspace）
crates/
  nebula-core/                 # 内核：无 UI 依赖，可在 Win/Linux/Android 编译
    src/config.rs              # CLI / 环境配置（data_dir, p2p_port, ui_port, nickname…）
    src/db.rs                  # rusqlite WAL 存储层（DbPool = Arc<Mutex<Connection>>）
    src/blob.rs                # SHA-256 CAS：<data_dir>/blobs/<sha256>
    src/discovery.rs           # UDP 广播 + 组播 + 127.0.0.1:48600..48609 独占回环槽位
    src/moments.rs             # Append-Only 事件日志 → 朋友圈聚合视图
    src/sync.rs                # Outbox 冲刷 + 版本向量 Gossip + 缺失 Blob 后台拉取
    src/server.rs              # Axum: /api/p2p/* /api/local/* /ws + 内嵌 ui/dist
    src/client.rs              # hyper 纯 Rust HTTP 客户端（不依赖 OpenSSL/ring）
    src/lib.rs                 # NebulaNode 统一运行时控制器
    examples/nebula-node.rs    # 无头节点（本地联调 / 浏览器调试）
    tests/e2e_nebula_cluster.rs# 三节点真实端到端测试
  nebula-tui/                  # ratatui + crossterm 终端客户端
  nebula-app/                  # Tauri v2 宿主（独立 workspace，GUI 默认；--tui 双模）
ui/dist/                       # 零构建依赖的单页前端，由 core include_str! 内嵌
.github/workflows/nebula-release.yml
```

## 2. 运行模型

* 一个 `NebulaNode` 同时启动：
  * P2P HTTP/WS 服务 `0.0.0.0:<p2p_port>`（0 = 系统分配）
  * 仅回环 UI 服务 `127.0.0.1:<ui_port>`（0 = 系统分配）
  * 两者共用同一个 `Router`，所以桌面 WebView、Android WebView、浏览器、TUI
    看到的 API/静态资源完全一致，且页面与 API **同源**。
* `PeerID` 首次启动随机生成 16 位 hex 并持久化在 `identity` 表，与 IP 完全解耦。
* Tauri 窗口先以隐藏状态加载内置 `launcher.html`；`setup` 内启动节点后，
  通过 Tauri IPC 命令 `nebula_info` 把真实 `http://127.0.0.1:<ui_port>` 交给
  launcher，再做顶层跳转（桌面端同时由 Rust 调用 `navigate`）。这样不依赖
  WebView 跨源 `fetch`，避开 Android 的 mixed-content / CORS 限制。
* Android 构建会额外注入 `network_security_config.xml`（允许 loopback 明文）
  与 `usesCleartextTraffic="true"`；内核启动失败时 launcher 会直接显示 IPC
  返回的错误信息，便于真机排障。

## 3. 发现协议（UDP, NEBULA_V1）

每 2 s 广播 JSON：

```json
{"protocol":"NEBULA_V1","packet_type":"ANNOUNCE","peer_id":"…","nickname":"…",
 "bio":"…","avatar_color":"#8b5cf6","p2p_port":52101,
 "moments_head_clock":42,"timestamp":1762590000000}
```

发送目标：`255.255.255.255:48600`、`239.255.48.60:48600`、每个非回环
IPv4 接口的定向 `/24` 广播、`127.0.0.1:48600..=48609`。

接收：
* LAN 广播/组播：`0.0.0.0:48600`（`SO_REUSEADDR`，多进程可同时监听）。
* 回环槽位：**不使用** `SO_REUSEADDR`，从 48600 起绑定第一个真正可用的
  `127.0.0.1` 端口 —— Windows 上多实例各自独占一个槽位，心跳扇出到全部
  10 个槽位，因此单机 N 个进程也能 100% 互相发现。
* 超过 8 s 无心跳 → `offline`；退出时发 `LEAVE`。
* 源 IP 选择：优先非回环地址（跨设备可路由），回环包不会覆盖已记录的 LAN IP。

## 4. 数据模型（SQLite）

`identity / peers / blobs / messages / moment_events` 五张表，见 `db.rs`。
* `messages.media_meta_json` 只保存元数据（缩略图 data URL、宽高、时长、16 段波形）。
* `moments` 使用确定性 ID：`{lamport:016}-{peer_id}-{rand:08x}`，`INSERT OR IGNORE`，
  仅 `rows_affected > 0` 时才触发再次传染（防广播风暴）。
* 会话 ID：大厅 `lobby`；私聊 `min(peerA,peerB):max(peerA,peerB)`。

## 5. P2P API

| Endpoint | 作用 |
|---|---|
| `POST /api/p2p/chat` | 单条实时消息，返回 `ack_msg_id` |
| `POST /api/p2p/sync` | Outbox 冲刷 + `known_moment_clocks` 版本向量对账 + 最近 100 条大厅消息 |
| `GET /api/p2p/blob/{sha256}` | 流式返回 CAS 二进制；接收端校验 SHA-256 后落盘 |

本地 UI：`/api/local/bootstrap`、`/api/local/messages`、`/api/local/blob`、
`/api/local/moments/post|interact`、`/api/local/profile`、`/ws`（实时事件总线）。

## 6. 可靠性语义

* 消息先落库（`queued`）→ 在线则立即 POST 并置 `delivered`；离线保持 `queued`。
* 每轮 `/api/p2p/sync` 携带发往该节点的 queued 消息（含自己未送达的大厅消息），
  对端 ack 后发送端更新为 `delivered`。
* Blob 仅按需拉取：消息/事件只带 `blob_id`，后台任务从源节点（或任一在线节点）
  拉取、校验、落盘，并推送 `BLOB_READY`。
* 朋友圈传染：A→B 同步后 A 下线，C 与 B 握手时用版本向量比对，B 会把 A 的
  POST/LIKE/COMMENT 及媒体一并代传给 C。

## 7. UI

* **单一 DOM 树**：`#nebula-app` 固定结构，仅通过 `data-tab` /
  `data-mobile-pane` / `data-viewport-sim` 与 CSS Container Queries 切换布局。
* 容器放在 `#sim-frame`，视口仿真器只改容器尺寸 → 与 `@media` 真实视口行为一致。
* 断点：手机 `<768`（底部导航 + 推拉栈）、平板 `768–1279`（64px 导航轨 +
  双栏 + 右侧抽屉 + 双列动态）、桌面 `>=1280`（72/320/flex/300 四区）。
* 草稿、录音状态、当前会话在任意旋转/缩放中保持（Composer 永不重建）。
* TUI：宽度 `>=100` 三栏，`<100` 单栏 + `F1/F2/F3`；键位见 `nebula-tui/src/lib.rs`。

## 8. 已知取舍

* TUI 的 `v` 发送合成 WAV 测试语音（不引入原生音频依赖）；`p` 发送本地图片路径。
* Android 多播锁未接入（未使用 Android 原生 API）；UDP 广播 + 回环仍可发现同网段节点。
* Tauri 的 Windows/Linux/Android 构建在 CI 完成；本机仅验证 core + TUI + Web。

## 9. 常用命令

```bash
cargo test -p nebula-core          # 三节点 e2e：发现/CAS/离线补发/朋友圈传染
cargo test -p nebula-tui           # TestBackend 渲染 + 全键位交互
cargo run -p nebula-core --example nebula-node -- --data-dir D:/nebula/n1 --ui-port 48610
cargo run -p nebula-tui -- --data-dir D:/nebula/t1 --name Nova
cargo build --release --manifest-path crates/nebula-app/Cargo.toml  # 需要 Tauri 平台依赖（CI 构建）
```
