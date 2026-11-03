<p align="center">
  <img src="assets/icon.png" alt="Guobeng Music Server" width="200">
</p>

<p align="center">
  图标原画 <a href="https://www.pixiv.net/artworks/110598995">無题</a>（pixiv）· 角色 <b>散兵</b>（《原神》）<br>
  角色版权归米哈游所有。项目名取自散兵的真名「<b>雷电国崩</b>」——<code>guobeng</code> 就是「国崩」的拼音
</p>

> **AI 屑作警告！讨厌 AI 的立即退出（？）**
>
> **本项目全由 AI 管理。** 代码、文档、构建脚本与仓库配置都由 AI 代理生成并维护，人工只负责提需求与验收。

# Guobeng Music Server

<p align="center">
  <b>简体中文</b> · <a href="README.en.md">English</a>
</p>

---

个人音乐库的**后端服务**。用 Rust 写成，编译产物是一个可执行文件，跑起来就是一套 JSON API。

它把「艺人 → 专辑 → 歌曲」三级结构、LRC 歌词、收藏列表、评论、播放历史与账户体系
整理成一份干净的 REST 契约。前端（网页 / 安卓 / 桌面 / 命令行）只面对 JSON 和静态媒体，
不需要知道数据库长什么样、文件存在哪。

- **技术栈**：Rust · axum · SQLite（单文件数据库）
- **契约**：`/api/v1`，所有响应统一 `{ code, message, data }`
- **部署**：一个二进制 + 一个 `config.json`，不需要 Python / Node / JVM / 外部数据库

## 主要功能

**曲库管理**

- 艺人、专辑、歌曲三级 CRUD，专辑与歌曲都带封面、简介等元数据
- **多艺人**：一首歌可挂多位艺人，按录入顺序展示；合作艺人会自动并入专辑艺人页，不会出现「合作曲在原唱主页看不到」的问题
- **专辑可见性**：公开 / 私有。私有专辑对非所有者表现为 **404**（不是 403，不泄漏存在性），歌曲、歌词、评论、收藏、历史全部继承该规则
- **音频上传**：服务端计算文件 MD5，重复上传直接 409 并告知已有歌曲 ID；单文件上限 1GB，支持 mp3 / flac / wav / m4a / aac / ogg / opus
- **歌曲排序**：拖拽重排，服务端自动压实 `order_index`，不会留空洞
- **外部音频**：也可以只存一个 `http(s)` 地址，不占服务器空间
- **外部专辑 ID**：`external_id` 全局唯一，从第三方曲库导入时可直接查重

**播放**

- `/media` 支持 **Range（206）** 与 HEAD，拖动进度条、断点续播、边下边播都可以
- 播放历史上报：记录进度、实际时长、是否听完（≥95% 自动标记完成），支持「继续播放这张专辑」

**歌词**

- LRC 文本存储，写入时校验时间戳格式，格式不对直接 400
- 支持一行多时间戳、`[ti:] [ar:] [al:] [by:] [offset:]` 标签
- 后端不抓第三方歌词，只存你给的那一份

**评论与收藏**

- 评论挂在歌曲上：主楼 + 一层回复，不允许多层嵌套
- 收藏必须归属自建列表，一首歌可同时属于多个列表；列表顺序、列表内歌曲顺序都能自定义
- **批量收藏检查**：一次查询最多 200 首，列表页一个请求就能把红心点亮

**账户与安全**

- 注册 / 登录 / 刷新 / 登出 / 修改密码
- access token 为 30 分钟 JWT；refresh token 7 天、**每次刷新都轮换**，服务端只存 SHA-256 摘要
- 修改密码会让该用户全部 refresh token 立刻失效
- `root` 账户仅允许本机登录（`127.0.0.1` / `::1`），远程持有其令牌按未登录处理
- 登录限流：同一 IP + 账号，5 分钟内失败 8 次锁定 5 分钟
- 角色分 `user` / `admin` / `root`，越权访问返回 403

**运维**

- `config.json` 缺键时按默认值补写回文件，用户能看到全部可配置项
- `allow_registration`、`allow_user_audio_upload` **热加载**，改完即时生效不用重启
- `/health` 健康检查（根路径，不在 `/api/v1` 下），`/docs` 一页接口速查

## 与同类项目的区别

拿 **Navidrome** 当对照——它是最常见的自托管音乐服务，也正好是本项目取向的另一端。

| 维度 | Guobeng Music Server | Navidrome |
|---|---|---|
| 产品形态 | 只有后端 API，前端你自己写 | 完整产品，自带 React Web UI |
| 协议 | 自有的 RESTful JSON 契约 | Subsonic / OpenSubsonic API |
| 客户端生态 | 只服务你自己写的客户端 | 现成的几十个手机 / 桌面 App 直接可用 |
| 入库方式 | HTTP 上传写入曲库，元数据走 API | 扫描本地音乐目录，读文件内嵌标签 |
| 转码 | 不提供，原文件直出 | ffmpeg 实时转码（可省流量） |
| 外部集成 | 无 | Last.fm / ListenBrainz scrobble、公开分享链接、多曲库 |
| 多艺人 | 一等公民，独立关联表 | 依 Subsonic 数据模型，以专辑艺人为主 |
| 数据模型 | 艺人 / 专辑 / 歌曲 + 评论 + 收藏列表 + 播放轨迹 | 面向 Subsonic 规范，每用户 starred / rating / playcount |

**一句话选型**

- 想开箱即用、手机上装个现成 App 就能听 → 用 Navidrome。
- 想拿一套干净的后端契约、前端完全自己掌控，或者要把音乐库嵌进自己的应用 → 用这个。

**刻意不做的事**：转码、scrobble、内置播放器、目录自动扫描、Subsonic 兼容。
这些不是没做完，是边界——每多一项，前端能拿到的自由度就少一分。这里只承诺一件事：
**把数据与权限管好，把接口说清楚**。

## 设计思路

**契约是产品，代码是实现。**
对外只暴露 `dto.rs` 定义的形状，数据库行长什么样（`models.rs`）不对外泄漏。
改表结构不该让前端跟着改，这是分层的意义。

**错误只有一种形状。**
所有响应（含错误）都是 `{ code, message, data }`；`code` 直接用 HTTP 状态码，成功为 0。
结构校验失败（缺字段、`page_size=101`）返回 422，并给出 `data.errors: [{field, message}]`，
前端可以直接把 `field` 映射到表单项，不用猜是哪一项错了。

**权限集中判定，不散落在 handler 里。**
「私有专辑对非所有者等于不存在」这条规则写在 `services.rs` 一处，
歌曲、歌词、评论、收藏、历史都复用它。规则只有一份，就不会出现某个接口忘了加过滤。

**事务边界在路由层。**
`services.rs` 只负责改对象，不 commit；什么时候提交由路由决定。这样一次请求要么整体成功，
要么整体回滚，不会留下半条数据。

**查询拼装在仓库层。**
`repo.rs` 提供统一的 SELECT 片段，`sqlutil.rs` 统一占位符生成，
避免同一个联表查询在十几个地方各写一遍、改字段时漏改一处。

## 代码结构

```
guobeng-music-server/
├── Cargo.toml
├── src/
│   ├── main.rs            # 启动、root 引导、中间件、路由装配
│   ├── settings.rs        # 进程级配置：命令行 / 环境变量 / config.json
│   ├── runtime_config.rs  # allow_* 开关热加载
│   ├── db.rs              # SQLite 连接池与建表
│   ├── models.rs          # 数据库行（内部）
│   ├── dto.rs             # 对外序列化结构（契约）
│   ├── payload.rs         # 请求体与字段校验
│   ├── extract.rs         # 鉴权提取器、分页、JSON 错误信封
│   ├── services.rs        # 可见性、权限、顺序、艺人关联
│   ├── hydrate.rs         # 批量装配嵌套 DTO（避免 N+1）
│   ├── repo.rs            # 统一 SELECT 片段
│   ├── sqlutil.rs         # IN 占位符生成
│   ├── security.rs        # PBKDF2、JWT、refresh token、登录限流
│   ├── media.rs           # 上传校验与落盘
│   ├── lyrics.rs          # LRC 校验
│   ├── timeutil.rs        # 时间格式
│   ├── error.rs           # 错误类型到信封的映射
│   ├── state.rs           # 跨请求共享状态
│   └── routes/            # 11 个路由模块 + mod.rs
│       ├── auth.rs  users.rs  artists.rs  albums.rs  tracks.rs
│       ├── comments.rs  favorites.rs  history.rs
│       └── files.rs  media.rs  health.rs  mod.rs
├── assets/icon.png        # 项目图标（版权归原作者，见文末）
├── docs/API.md            # 接口契约（前端必读）
└── .github/workflows/     # 云端构建：Windows x64 / Linux x64 / Linux arm64
```

## 构建与运行

需要 Rust 工具链（1.80+）。

```bash
cargo build --release

./target/release/guobeng-music-server                    # 默认 127.0.0.1:8000，数据存 ./data
./target/release/guobeng-music-server --host 0.0.0.0     # 对外监听
./target/release/guobeng-music-server --port 9000
./target/release/guobeng-music-server --base-dir D:/music --data-dir D:/music/data
./target/release/guobeng-music-server --init-only        # 只建库、创建 root 后退出
```

也可以双击 `build_user.bat`，它会询问目标平台（Windows / Linux / macOS）与架构（x86_64 / arm64），
缺交叉工具链时会直接告诉你缺什么。

首次启动会创建 `data/music.db` 与 root 账户，随机密码写入 `data/root_password.txt` 并打印到终端。
root 只允许本机登录。

**云端构建**：`.github/workflows/build.yml` 在 GitHub 的机器上产出三个平台的可执行文件，
本地不需要装任何交叉工具链。触发方式：Actions → build → Run workflow，或推一个 `v*` 标签。

| 产物 | 平台 | 说明 |
|---|---|---|
| `guobeng-music-server-windows-x86_64.exe` | Windows x64 | 静态链接 C 运行库，不需要装 VCRUNTIME |
| `guobeng-music-server-linux-x86_64` | Linux x64 | musl 完全静态，不依赖目标机 glibc |
| `guobeng-music-server-linux-arm64` | Linux arm64 | 同上，交叉编译产出 |

两个 Linux 产物是完全静态的：CentOS 7、Ubuntu 18.04、Alpine、Android 的 Termux 都不需要额外装库。
代价是不走系统的 glibc 安全更新，libc 有漏洞时需要重新编译一次。
在 Android 上请在 `$HOME` 下运行（`/sdcard` 是 noexec 挂载）。

### 配置

`config.json` 在启动目录（可用 `--base-dir` 指定），首次启动自动生成：

```json
{
  "allow_registration": true,
  "allow_user_audio_upload": true,
  "backend_port": 8000,
  "host": "127.0.0.1"
}
```

- 两个 `allow_*` 开关**立即生效**，无需重启；root / admin 不受上传开关限制
- `backend_port` 与 `host` 只在启动时读取，改完必须重启
- `host` 填 `0.0.0.0` 才对外监听，默认只接受本机访问
- 端口优先级：`--port` > `MUSIC_SERVER_PORT` > `config.json` > 8000
- 地址优先级：`--host` > `MUSIC_SERVER_HOST` > `config.json` > `127.0.0.1`
- 缺键时按默认值补写回文件；已有值不会被覆盖，`--port 9000` 跑一次不会把默认端口固化

其他环境变量：`MUSIC_SERVER_SECRET_KEY`、`MUSIC_SERVER_ADMIN_ONLY_CREATE`、
`MUSIC_SERVER_CORS_ORIGINS`、`MUSIC_SERVER_DEBUG`、`MUSIC_SERVER_DATA_DIR`、
`MUSIC_SERVER_ENABLE_DOCS`。

## 前端对接

**完整契约：[`docs/API.md`](docs/API.md)** —— 每个接口的入参、出参、权限、错误码都在里面。
后端跑起来后，`http://127.0.0.1:8000/docs` 也有一页速查。

开始写之前，三件事先记住：

**1. 所有响应同一个信封**

```json
{ "code": 0, "message": "ok", "data": { } }
```

`code` 是 HTTP 状态码，成功为 0。数据永远在 `data` 里，不要在响应根上找字段。

**2. 认证走 Bearer，刷新会轮换**

```bash
# 登录拿一对令牌
curl -X POST http://127.0.0.1:8000/api/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"account":"root","password":"..."}'

# 之后每个请求带上 access token
curl http://127.0.0.1:8000/api/v1/albums \
  -H "Authorization: Bearer <access_token>"
```

access token 30 分钟过期，用 refresh token 换新的；**旧的 refresh token 会立即作废**，
所以刷新失败时不要拿旧的重试，直接跳登录。401 才算掉线，403 是权限问题、不要清登录态。

**3. 422 会告诉你是哪个字段**

```json
{
  "code": 422,
  "message": "请求参数校验失败",
  "data": { "errors": [{ "field": "account", "message": "缺少必填字段" }] }
}
```

`field` 直接对应表单字段名，可以逐项标红。

**播放器接入**：音视频走 `/media/*`，支持 Range，`<audio>` / `AVPlayer` / ExoPlayer 给 URL 就能播，
不需要自己做分片。播放进度调 `POST /history` 上报，下次进专辑页用
`GET /history/album/{id}/last` 恢复现场。

## 图标版权

项目图标 `assets/icon.png` **版权归原作者 無题 所有**。

- 原图链接：<https://www.pixiv.net/artworks/110598995>
- 图中角色为《原神》的**散兵**，角色版权归米哈游（HoYoverse）所有
- 使用范围：仅作本项目标识与文档展示，不用于商业用途，未对画面作二次修改
- 本项目与该图片不主张任何权利，图片权利完全属于原作者

**如有侵权，请联系我们删除**：在仓库提交 issue 并注明「图标版权」，
说明权利人身份与诉求即可；核实后会立即移除 `assets/icon.png` 及全部引用。

## 许可

以 **MIT** 协议发布，全文见 [LICENSE](LICENSE)。任何人都可以自由使用、修改、分发、商用，
无需申请授权；唯一要求是保留版权声明与许可声明，作者不承担任何担保责任。

项目图标 `assets/icon.png` **不在 MIT 授权范围内**，版权归属原作者，见上一节。
