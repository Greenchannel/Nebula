# Nebula（星云）

> 无中心服务器、纯局域网的 P2P 即时通讯与去中心化动态平台。
> Windows / Linux 桌面 GUI + TUI，Android GUI，浏览器调试端 —— 全部共用同一套内核、协议与前端。

* **零服务器**：UDP 自发现（广播 + 组播 + 回环端口池），节点间直接 HTTP/WS。
* **PeerID 与 IP 解耦**：换 DHCP 地址、换 Wi-Fi 不丢好友、不拆会话。
* **富媒体**：文本 / 图片 / 语音；媒体进入 SHA-256 CAS，消息只带 `blob_id` + ≤4KB 缩略图 + 16 段波形。
* **离线可靠投递**：Outbox 队列 + 上线自动补发。
* **星云圈**：Append-Only 事件日志 + 版本向量 Gossip，A→B→C 代传，作者离线也能最终一致。
* **四阶 UI**：手机 / 平板 / 桌面 / 终端；单 DOM 树，旋转缩放零状态丢失。

---

## 1. 环境要求

| 目标 | 依赖 |
|---|---|
| core + TUI（本机已验证） | Rust stable（Windows 可用 `x86_64-pc-windows-gnu` + MinGW gcc） |
| 桌面 GUI（Tauri v2） | Windows: MSVC Build Tools + WebView2；Linux: `libwebkit2gtk-4.1-dev` 等 |
| Android APK | JDK 17、Android SDK 34、NDK 27、Rust android targets（建议直接交给 CI） |

本机（Windows 11）Rust 安装位置：

```text
RUSTUP_HOME=D:\rust\rustup
CARGO_HOME=D:\rust\cargo
PATH 追加 D:\rust\cargo\bin
```

---

## 2. 构建与测试

```bash
# 内核 + TUI（本机）
cargo test -p nebula-core      # 三节点 e2e：发现 / CAS / 离线补发 / 朋友圈传染
cargo test -p nebula-tui       # TestBackend 渲染 + 全键位交互回归
cargo build --release -p nebula-tui

# Tauri GUI（Linux / Windows / Android，CI 已配置）
cargo build --release --manifest-path crates/nebula-app/Cargo.toml
```

> 根 workspace 只包含 `nebula-core` + `nebula-tui`；`nebula-app` 是独立的 Tauri
> workspace（标准 `src-tauri` 布局）。这样本地 `cargo test` 永远不会解析/下载
> Tauri 的数百个依赖。CI 用 `--manifest-path` 构建 GUI。
> 本机（Windows GNU）已验证 core + TUI + Web UI；Tauri 桌面与 Android 构建由
> `.github/workflows/nebula-release.yml` 在 CI 完成。

---

## 3. 本地多开体验

每个节点必须使用**独立的 `--data-dir`**；P2P/UI 端口可写 `0` 自动分配，也可显式指定。

### 3.1 两个 GUI + 一个 TUI + 浏览器

```powershell
# 节点 A（GUI）— UI 端口 48610
nebula.exe --data-dir D:\nebula\a --ui-port 48610 --name Alpha

# 节点 B（GUI）— UI 端口 48611
nebula.exe --data-dir D:\nebula\b --ui-port 48611 --name Beta

# 节点 C（TUI，独立终端）
nebula-tui.exe --data-dir D:\nebula\c --name Gamma

# 可选：浏览器同时打开（与 GUI 共享同一节点的数据与实时总线）
start http://127.0.0.1:48610/
start http://127.0.0.1:48611/
```

启动日志会打印实际 UI 端口；`--tui` 也可以直接交给主程序：

```powershell
nebula.exe --tui --data-dir D:\nebula\c --name Gamma
```

> Windows 下 `nebula.exe` 采用 windowed 子系统：双击不弹黑框；
> `--tui` 会先 `AttachConsole(ATTACH_PARENT_PROCESS)`，失败则 `AllocConsole()`。

### 3.2 无头 Web 节点（开发/联调）

```bash
RUST_LOG=nebula_core=info cargo run -p nebula-core --example nebula-node -- \
  --data-dir D:/nebula/web1 --p2p-port 52101 --ui-port 48610 --name Web-1
```

### 3.3 建议体验流程

1. 启动 A、B → 1 秒内两侧会话列表出现对方（`peers.status = online`）。
2. A 向 B 发文字、图片（拖拽 / 粘贴 / 📎）、语音（按住 🎙️，上滑取消）：
   * 原图与语音自动从 A 的 CAS 流式拉到 B，SHA-256 校验一致后由模糊缩略图切换为高清。
3. A 关闭 B → 给 B 发消息，气泡显示 `◷ 离线排队中`；重新启动 B（同一 `--data-dir`）→
   自动补发并变为 `✓✓ 已送达`。
4. A 发布带图动态 → B 点赞 + 评论 → 关闭 A → 启动 C：
   * C 与 B 握手后通过版本向量对账，拿到 A 的动态、图片 Blob、B 的赞与评论。
5. 视口仿真器切换 手机 / 平板竖屏 / 平板横屏 / 桌面：输入框草稿、录音状态、当前会话保持不变。
6. TUI 宽度 ≥100 列为三栏，拖窄到 <100 自动折叠为单栏 + `F1/F2/F3`。

### 3.4 TUI 键位

| 键 | 作用 |
|---|---|
| `↑/↓` | 切换会话 / 动态 / 节点 |
| `i` | 输入并发送文字（星讯）/ 发布动态（星云圈） |
| `p` | 输入本地图片路径并发送（CAS） |
| `v` | 发送合成测试语音（16 段波形） |
| `n` | 发布新动态 |
| `l` / `c` | 给选中动态点赞 / 评论 |
| `PgUp/PgDn` | 滚动消息；`End` 回到底部 |
| `1/2/3` 或 `F1/F2/F3` | 切换面板 |
| `q` / `Ctrl+C` | 优雅退出（广播 LEAVE） |

---

## 4. GitHub Actions 发布

> ⚠️ 工作流必须位于 **GitHub 仓库根目录** 的 `.github/workflows/`。
> 当前 `Nebula/` 位于 `D:\coding` 这个大仓库的子目录中；请把 `Nebula/` 作为独立仓库
> 推到 GitHub（或把该目录作为仓库根）。

### 4.1 触发

* `push` 到 `main` / `v*` 标签
* `pull_request`
* `workflow_dispatch`（手动）

### 4.2 三个并行 Job 与产物

| Job | 产物 |
|---|---|
| `build-linux` (ubuntu-22.04) | `nebula-linux-gui`、`nebula-linux-tui` |
| `build-windows` (windows-latest, MSVC) | `nebula-windows-gui.exe`、`nebula-windows-tui.exe` |
| `build-android` (ubuntu-22.04) | `nebula-android-arm64-signed.apk` |

Android 流水线固定顺序（避免坑点 10）：

1. `cargo tauri android init`（若 `gen/android` 不存在）
2. 注入 `INTERNET / ACCESS_NETWORK_STATE / ACCESS_WIFI_STATE /
   CHANGE_WIFI_MULTICAST_STATE / RECORD_AUDIO` + `android:usesCleartextTraffic="true"`
3. `cargo tauri android build --apk --target aarch64`
4. **先** `zipalign -f -v -p 4` → **再** `apksigner sign` → `apksigner verify -v`

### 4.3 签名 Secrets（可选）

未配置时自动生成自签名发布证书（口令 `nebula123456`，仅适合内测分发）：

```text
ANDROID_KEYSTORE_BASE64       # base64 of release .jks
ANDROID_KEYSTORE_PASSWORD
ANDROID_KEY_ALIAS
ANDROID_KEY_PASSWORD
```

正式分发请务必妥善离线备份 keystore：换签名后无法覆盖安装升级。

### 4.4 Android 安装

1. 下载 `nebula-android-arm64-signed.apk`
2. 手机允许「安装未知来源应用」
3. 安装并打开 → 首次启动自动生成 PeerID
4. 与桌面端处于同一 Wi-Fi 即可互相发现
5. 首次录音请在系统弹窗中允许麦克风；若 WebView 未触发授权请求，
   应用会自动降级为合成测试语音（其余功能不受影响）。

---

## 5. 常见问题

* **同一台机器多个节点互相看不见？**
  内核会把心跳发往 `127.0.0.1:48600..48609` 的 10 个槽位，每个进程独占一个槽位，
  Windows/Linux 均可在 1 秒内互相发现；请确认各节点 `--data-dir` 不同。
* **跨设备发现失败？**
  检查 Windows 防火墙是否放行 UDP 48600 与 P2P TCP 端口；部分企业 Wi-Fi 禁止客户端互访。
* **端口被占用？**
  用 `--p2p-port 0 --ui-port 0` 自动分配；启动日志会打印真实端口。
* **网页里图片一直是模糊缩略图？**
  原图尚在后台 CAS 拉取；对端需在线且拥有该 Blob。状态会在 `BLOB_READY` 后自动切换。
* **TUI 语音是合成的？**
  为避免 `alsa-sys` 等原生音频依赖，TUI 的 `v` 生成标准 WAV 测试语音；
  GUI/Web 使用真实 `MediaRecorder`，无麦克风时同样自动降级为合成语音。
