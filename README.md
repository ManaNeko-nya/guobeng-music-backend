> **本项目全由 AI 管理。** 代码、文档、构建脚本与仓库配置都由 AI 代理生成并维护，人工只负责提需求与验收。

# MusicServer · 个人音乐库（Rust 版）

个人音乐媒体服务器：**艺人 → 专辑 → 歌曲**、LRC 歌词、收藏列表、评论、播放历史与账户系统。
Rust + axum + SQLite 实现，编译产物是单个可执行文件，运行不需要 Python 或任何运行时环境。

- JSON API 契约与原 Python 版一致，见 [`docs/API.md`](docs/API.md)
- 旧的 Python 实现不进仓库（`.gitignore` 排除了 `python-legacy/`），只在本机留档参考

## 构建与运行

Windows 上不依赖 MSVC：工具链用 `stable-x86_64-pc-windows-gnu`，链接器用 MinGW-w64。
本机已把默认工具链切到 gnu 目标，`D:\App\MinGW\bin` 也已加入用户 PATH，因此直接构建即可：

```bash
cargo build --release
# 产物：target/release/music-server.exe

./music-server                 # 默认 127.0.0.1:8000，数据目录 ./data
./music-server --host 0.0.0.0 --port 8000
./music-server --base-dir D:/music --data-dir D:/music/data
./music-server --init-only     # 只建库、创建 root 后退出
```

也可以双击 `build_user.bat`：它会问目标平台（Windows / Linux / macOS）与架构（x86_64 / arm64），
按所选三元组构建，产物在 `target/<三元组>/release/` 下。
注意只有 Windows x86_64 是本机可直接构建的；另外几个需要对应平台的交叉编译工具链，
脚本缺工具时会说明缺什么，而不是甩一串链接错误。

换一台机器时，先装 MinGW-w64 并把它加进 PATH，再执行：

```bash
rustup target add x86_64-pc-windows-gnu
rustup toolchain install stable-x86_64-pc-windows-gnu
rustup default stable-x86_64-pc-windows-gnu
cargo build --release
```

Linux / macOS 直接 `cargo build --release` 即可，产物是 `target/release/music-server`。

### 云端构建（推荐）

`.github/workflows/build.yml` 会在 GitHub 的机器上编出三个平台的可执行文件，
本地不需要装任何交叉工具链：

| 产物 | 平台 | 说明 |
|---|---|---|
| `music-server-windows-x86_64.exe` | Windows x64 | 静态链接 C 运行库，不需要装 VCRUNTIME |
| `music-server-linux-x86_64` | Linux x64 | musl 静态链接，不依赖目标机 glibc，任何发行版通用 |
| `music-server-linux-arm64` | Linux arm64 | 同上，交叉编译产出 |

两个 Linux 产物都是 **完全静态**：CentOS 7、Ubuntu 18.04、Alpine、以及 Android 的 Termux
都不需要额外装库。代价是不走系统的 glibc 安全更新，libc 有漏洞时要重新编译一次。
Android 上请在 `$HOME` 下运行（`/sdcard` 是 noexec 挂载），详见脚本 `termux_install.sh`。

触发方式：仓库页 **Actions** → 左侧 **build** → **Run workflow**；
或者推一个 tag（`git tag v1.0 && git push origin v1.0`）。
跑完后在该次运行页底部的 **Artifacts** 里下载。

首次启动创建 `data/music.db` 与 root 账户，随机密码写入 `data/root_password.txt` 并打印到终端。
root 只允许本机登录（`127.0.0.1` / `::1`）。

- API：`http://127.0.0.1:8000/api/v1`
- 健康检查：`/health`；接口说明页：`/docs`
- 媒体：`/media/*`，支持 Range（206）与 HEAD，可拖动播放

## 目录结构

```
music_rust/
├── Cargo.toml
├── src/
│   ├── main.rs            # 启动、root 引导、中间件、路由装配
│   ├── settings.rs        # 命令行 / 环境变量 / config.json
│   ├── runtime_config.rs  # allow_* 热加载
│   ├── db.rs              # SQLite 连接池与表结构
│   ├── models.rs          # 数据库行
│   ├── dto.rs             # 对外序列化结构（契约）
│   ├── payload.rs         # 请求体与校验
│   ├── extract.rs         # 鉴权 / 分页 / JSON 错误信封
│   ├── services.rs        # 可见性、权限、顺序、艺人关联
│   ├── hydrate.rs         # 批量装配嵌套 DTO
│   ├── repo.rs            # 统一 SELECT 片段
│   ├── security.rs        # PBKDF2、JWT、refresh token、登录限流
│   ├── media.rs           # 上传校验与落盘
│   ├── lyrics.rs          # LRC 校验
│   └── routes/            # auth/users/artists/albums/tracks/comments/favorites/history/files/media
├── docs/API.md            # 接口契约
├── .github/workflows/     # 云端构建（Windows / Linux x64 / Linux arm64）
└── data/                  # 运行时：music.db、media/、secret.key、root_password.txt
```

## 配置

`config.json` 在启动目录（可用 `--base-dir` 指定），首次启动自动生成：

```json
{
  "allow_registration": true,
  "allow_user_audio_upload": true,
  "backend_port": 8000,
  "host": "127.0.0.1"
}
```

- 两个 `allow_*` 业务开关**立即生效**，无需重启；root / admin 不受上传开关限制；
- `backend_port` 与 `host` 只在启动时读取，改完必须重启；
- `host` 填 `0.0.0.0` 才对外监听，默认只接受本机访问；
- 端口优先级：`--port` > 环境变量 `MUSIC_SERVER_PORT` > `config.json` > 8000；
- 地址优先级：`--host` > 环境变量 `MUSIC_SERVER_HOST` > `config.json` > `127.0.0.1`；
- 缺键时按默认值补写回文件，所以文件里能看到全部可配置项；已有值不会被覆盖。
  补写的是默认值而非本次启动的覆盖值，`--port 9000` 跑一次不会把默认端口改掉。

环境变量：`MUSIC_SERVER_HOST`、`MUSIC_SERVER_PORT`、`MUSIC_SERVER_SECRET_KEY`、
`MUSIC_SERVER_ADMIN_ONLY_CREATE`、`MUSIC_SERVER_CORS_ORIGINS`、`MUSIC_SERVER_DEBUG`、
`MUSIC_SERVER_DATA_DIR`、`MUSIC_SERVER_ENABLE_DOCS`。

## 主要行为

- **权限**：公开内容免登录浏览；艺人/专辑由创建者管理，管理员不受限；
  私有专辑对非所有者表现为 404，歌曲、歌词、评论、收藏、历史都继承该可见性。
- **多艺人**：`track_artists` / `album_artists` 是唯一事实来源，`albums.artist_id` 仍是主艺人；
  歌曲的合作艺人会自动并入专辑艺人。
- **音频**：上传计算 MD5，重复上传返回 409；`/media` 支持 Range；单文件上限 1GB。
- **歌词**：只接受带时间戳的 LRC，最多 200000 字符 / 10000 行。
- **令牌**：JWT access 30 分钟；refresh 7 天、服务端只存 SHA-256、每次刷新轮换。
- **登录限流**：同一 IP + 账号 5 分钟失败 8 次锁定 5 分钟。

## 与 Python 版的差异

| 项目 | 说明 |
|---|---|
| 数据 | 全新 SQLite 库，不读取旧 Python 版的 `music.db`；旧媒体文件也不复用 |
| 日志 | 只输出到控制台（stdout/stderr），不写 `data/logs/server.log`；需要留档自行重定向 |
| 文档 | 不再内置 Swagger，`/docs` 是一页接口说明；完整契约在 `docs/API.md` |
| 运维命令 | 不提供 `--seed-demo` / `--refresh-demo` / `--backfill-md5`（新库没有历史 MD5 数据） |
| 网页端 | 未包含；原 FastAPI 网页端需要 Python 才能运行，旧代码未入库 |
