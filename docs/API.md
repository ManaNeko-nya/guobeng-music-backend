# Guobeng Music Server API 文档

> 版本：2.0.0 · 基础路径 `/api/v1` · 接口说明页：`http://<backend>/docs`
>
> 音乐库层级：**艺人 Artist → 专辑 Album → 歌曲 Track**；歌词以 LRC 文本保存在歌曲上。
> 前端服务只通过本文档的 JSON API 访问后端；安卓端同样直接调用这些接口。

## 1. 通用约定

| 项目 | 说明 |
|---|---|
| Base URL | `http://127.0.0.1:8000/api/v1`；`/health`、`/media`、`/docs` 在**根路径**，不带这个前缀 |
| 编码 | 请求与响应均为 UTF-8 JSON（上传为 `multipart/form-data`） |
| 认证 | `Authorization: Bearer <access_token>` |
| 时间 | ISO 8601，UTC（如 `2026-10-20T05:30:00`）；日期为 `YYYY-MM-DD` |
| 分页 | `page` 从 1 开始，`page_size` 1–100（默认 20） |
| 字段命名 | `snake_case` |

### 1.1 统一响应

所有接口（包括错误）都返回同一结构：

```json
{ "code": 0, "message": "ok", "data": { } }
```

- `code = 0` 表示成功，`data` 为业务数据；
- `code != 0` 表示失败，`message` 可直接展示给用户，`data` 通常为 `null`。

### 1.2 HTTP 状态码

| 状态码 | 含义 | 典型场景 |
|---|---|---|
| 200 | 成功 | 查询、更新、删除 |
| 400 | 参数业务校验失败 | 密码强度不足、歌词格式错误 |
| 401 | 未登录 / 令牌失效 / root 非本机 | 缺少或过期 token |
| 403 | 无权限 | 修改他人内容、普通用户上传被关闭 |
| 404 | 资源不存在或不可见 | 私有专辑对非所有者表现为不存在 |
| 409 | 资源冲突 | 用户名/艺人名重复、重复收藏、列表重名 |
| 422 | 请求体/查询参数结构校验失败 | 缺少必填字段、`page_size=101` |
| 429 | 频率限制 | 登录失败次数过多 |
| 500 | 服务端异常 | 未捕获错误 |
| 503 | 后端不可用 | 前端代理无法连接后端 |

### 1.3 认证模型

- **access token**：JWT，有效期 30 分钟，放在 `Authorization: Bearer` 头；
- **refresh token**：不透明随机串，有效期 7 天，仅存 SHA-256 摘要，**每次刷新都会轮换**；
- 登出吊销 refresh token；`logout-all` 吊销该用户全部令牌；
- **修改密码会使该用户全部 refresh token 失效**，旧设备需要重新登录；
- **root 账户仅允许本机**（`127.0.0.1` / `::1`）登录、刷新令牌以及携带其令牌的请求；远程访问按未登录处理。

### 1.4 权限记号

| 记号 | 含义 |
|---|---|
| 公开 | 无需认证 |
| 登录 | 需要有效 access token |
| 艺人所有者 | 艺人创建者本人，或 `root` / `admin` |
| 专辑所有者 | 专辑创建者本人，或 `root` / `admin` |
| 歌曲管理 | 歌曲所属专辑的所有者，或 `root` / `admin` |

**可见性**：`Album.is_public=true` 对所有人生效；私有专辑仅所有者与管理员可见。
歌曲、评论、歌词、收藏、历史都继承所属专辑的可见性。

---

## 2. 账户 `/auth`、`/users`

账户接口与上一版一致：

| 接口 | 方法 | 权限 | 说明 |
|---|---|---|---|
| `/auth/register` | POST | 公开 | `{username, password, nickname?}`；用户名为 3–32 位字母/数字/`_.-` |
| `/auth/login` | POST | 公开 | `{account, password}` |
| `/auth/refresh` | POST | 公开 | `{refresh_token}`，返回新的令牌对 |
| `/auth/logout` | POST | 登录 | `{refresh_token?}`；不传吊销全部 |
| `/auth/logout-all` | POST | 登录 | 吊销全部刷新令牌 |
| `/users/me` | GET | 登录 | 本人资料 |
| `/users/me` | PATCH | 登录 | `{nickname?, bio?, avatar_url?}`；显式传 `null` 可清空 bio/avatar |
| `/users/me/password` | POST | 登录 | `{old_password, new_password}` |
| `/users/{uuid}` | GET | 公开 | 用户公开资料 |

登录/注册返回：

```json
{
  "user": { "id": "<uuid>", "username": "alice", "nickname": "爱丽丝",
            "avatar_url": null, "bio": null, "role": "user", "created_at": "..." },
  "tokens": { "access_token": "eyJ...", "refresh_token": "3Qk...", "token_type": "bearer",
              "expires_in": 1800, "refresh_expires_in": 604800 }
}
```

---

## 3. 艺人 `/artists`

### 3.1 艺人对象

```json
{
  "id": 1, "name": "星野结衣", "cover_url": "/media/covers/x.png",
  "bio": "独立流行唱作人", "album_count": 2, "track_count": 18,
  "created_at": "2026-10-20T05:30:00", "updated_at": "2026-10-20T05:30:00"
}
```

`album_count` 统计艺人是「主艺人或合作艺人」的全部专辑，`track_count` 统计其参与的歌曲（含合作曲）。
艺人详情额外包含 `owner_id`（uuid）与 `albums`（当前用户可见的专辑数组，同样含合作专辑）。

### 3.2 接口

| 接口 | 方法 | 权限 | 说明 |
|---|---|---|---|
| `/artists` | GET | 公开 | 参数：`page`、`page_size`、`q`、`sort=name|created_at|albums` |
| `/artists/mine` | GET | 登录 | 我创建的艺人 |
| `/artists/{id}` | GET | 公开 | 详情 + 可见专辑（含合作专辑） |
| `/artists/{id}/tracks` | GET | 公开 | 该艺人参与的歌曲，分页 `page`/`page_size`，`sort=release|title`；含合作曲 |
| `/artists` | POST | 登录 | `{name, cover_url?, bio?}`；名称唯一，重名 409 |
| `/artists/{id}` | PATCH | 艺人所有者 | 可改 `name` / `cover_url` / `bio`；显式 `null` 清空封面或简介 |
| `/artists/{id}` | DELETE | 艺人所有者 | 级联删除专辑、歌曲、评论、收藏、历史与本地媒体文件 |

`--admin-only-create` / `MUSIC_SERVER_ADMIN_ONLY_CREATE=1` 时，只有管理员可创建艺人。

---

## 4. 专辑 `/albums`

### 4.1 专辑对象

```json
{
  "id": 1, "artist_id": 1, "title": "潮汐来信",
  "cover_url": "/media/covers/x.png", "description": "……",
  "album_type": "album", "release_year": 2025, "release_date": "2025-04-18",
  "score": 8.8, "tags": ["流行", "独立"], "track_count": 6,
  "view_count": 1200, "is_public": true,
  "external_id": "965291",
  "created_at": "...", "updated_at": "...",
  "artist": { "id": 1, "name": "星野结衣", "...": "..." },
  "artists": [{ "id": 1, "name": "星野结衣", "...": "..." },
              { "id": 4, "name": "费玉清", "...": "..." }]
}
```

`artist_id` / `artist` 是专辑的主艺人（唯一外键），`artists` 是参与这张专辑的全部艺人
（按顺序，首位即主艺人）。合作专辑两个字段会同时返回，展示用 `artists`。
`album_type`：`album` 专辑 / `ep` EP / `single` 单曲 / `compilation` 合辑 / `ost` 原声。
`score`：0–10。标签是唯一分类维度，客户端内置流派列表，后端不提供全表标签聚合接口。

专辑详情额外包含 `owner_id`（uuid）、`is_public` 与 `tracks`（歌曲数组）。

### 4.2 列表与搜索

| 接口 | 方法 | 权限 | 参数 |
|---|---|---|---|
| `/albums` | GET | 公开 | `page` `page_size` `q` `sort` `artist_id` `tag` `year` `album_type` |
| `/albums/search` | GET | 公开 | 同上（与 `/albums` 等价） |
| `/albums/mine` | GET | 登录 | `page` `page_size`；返回含私有专辑 |
| `/albums/recommendations` | GET | 公开 | `limit` 1–50（默认 12）`seed` `exclude`（逗号分隔 ID） |

`sort`：`created_at`（默认 `updated_at` 请显式传）/ `updated_at` / `views` / `score` / `release` / `title`。
`q` 模糊匹配专辑标题、简介、艺人名、标签；`%`/`_` 会被转义。
`artist_id` 过滤同时匹配「主艺人」与「合作艺人」，结果取并集。
列表/搜索只返回公开专辑；私有专辑通过 `/albums/mine` 或详情（所有者）读取。

```json
{ "items": [], "total": 9, "page": 1, "page_size": 20, "total_pages": 1 }
```

### 4.3 详情与维护

| 接口 | 方法 | 权限 | 说明 |
|---|---|---|---|
| `/albums/{id}` | GET | 公开/所有者 | `count_view=false` 不增加播放量（管理端使用） |
| `/albums/by-external/{external_id}` | GET | 公开/所有者 | 按外部标识取专辑（含 `tracks`）；不存在返回 `404`，导入工具据此判断新建还是复用 |
| `/albums` | POST | 登录 + 艺人所有者 | `{artist_id, title, cover_url?, description?, album_type?, release_date?, release_year?, score?, tags?, is_public?, external_id?, artist_ids?}` |
| `/albums/{id}` | PATCH | 专辑所有者 | 字段均可选；显式传 `null` 可清空简介/封面/评分/发行日期/标签；`artist_id` 不可修改 |
| `/albums/{id}` | DELETE | 专辑所有者 | 级联删除歌曲、评论、收藏、历史与本地媒体文件 |

`release_date` 存在时 `release_year` 自动取日期年份；显式传 `release_date: null` 且未传 `release_year` 时年份也清空。

`external_id` 是外部来源（酷狗等）的专辑唯一标识：同一 `external_id` 全局只允许一张专辑，
重复创建返回 `409`；为空表示没有外部来源。专辑对象（列表/详情）都会带回该字段。

`artist_ids` 是专辑的完整艺人列表（可选）：`artist_id` 仍是主艺人，传了 `artist_ids` 时会
一并建立 `album_artists` 关联（主艺人自动补在首位）。上传歌曲时，歌曲的艺人会自动并入专辑艺人，
所以合作曲的歌手也会出现在这张专辑的艺人列表里。

---

## 5. 歌曲 `/albums/{id}/tracks`、`/tracks`

### 5.1 歌曲对象

```json
{
  "id": 10, "album_id": 1, "title": "潮汐来信", "order_index": 0,
  "audio_url": "/media/audio/x.wav", "file_size": 264644,
  "file_md5": "d83a7748ec0557d843d1903a36b6cc06",
  "duration_sec": 6.0, "has_lyrics": true, "comment_count": 2,
  "artists": [{ "id": 3, "name": "周杰伦", "cover_url": null, "album_count": 12 }],
  "created_at": "...", "updated_at": "..."
}
```

`artists` 是歌曲的参与艺人（按录入顺序），合作曲会有多项；老数据没有关联时为空数组，
此时展示层回退到 `album.artist`。写入时用 `artist_ids` 指定，服务端按顺序建立关联。

`/tracks` 列表与 `/tracks/{id}` 返回 `TrackWithAlbum`：在歌曲对象上增加 `album`（含 `artist`）。
`audio_url` 可以是 `/media/...` 或外部 `http(s)` 地址；创建后不可修改。
`duration_sec` 可为空；上传时可由客户端提供，播放历史上报时会用播放器实际时长补齐。
`file_md5` 是服务器在保存音频时计算的文件指纹；用于跨歌单/重复导入去重。

### 5.2 接口

| 接口 | 方法 | 权限 | 说明 |
|---|---|---|---|
| `/albums/{album_id}/tracks` | GET | 公开/所有者 | 按 `order_index` 升序返回 `TrackOut[]` |
| `/albums/{album_id}/tracks` | POST | 专辑所有者 | `{title, audio_url?, duration_sec?, order_index?, artist_ids?}` |
| `/albums/{album_id}/tracks/upload` | POST | 专辑所有者 + 上传开关 | multipart：`file`、`title?`、`duration_sec?`、`order_index?`、`artist_ids`（可重复） |
| `/albums/{album_id}/tracks/reorder` | POST | 专辑所有者 | `{track_ids: [...]}`，必须包含该专辑全部歌曲 ID |
| `/tracks` | GET | 公开/登录 | `q` `album_id` `artist_id` `md5` `sort=created_at|title|release`；登录后包含自己可见的私有专辑歌曲 |
| `/tracks/{id}` | GET | 公开/所有者 | 含专辑与艺人 |
| `/tracks/{id}` | PATCH | 歌曲管理 | 仅可改 `title` |
| `/tracks/{id}` | DELETE | 歌曲管理 | 级联删除评论/收藏/历史与本地音频文件，并压实顺序 |

**参与艺人**：`artist_ids` 是必填的艺人 ID 列表（至少一位），顺序即展示顺序，重复 ID 自动去重；
任一项不存在返回 `400`。上传接口通过重复表单字段传值（`artist_ids=1&artist_ids=2`），
JSON 接口直接传数组。歌曲与艺人是独立关联表（`track_artists`），不占用专辑的 `artist_id`：
专辑的主艺人仍由 `albums.artist_id` 表示，歌曲的其余艺人只挂在歌曲上，
同时会自动并入专辑艺人（`album_artists`），确保合作艺人主页能看到这张专辑。

**按艺人筛选**：`GET /tracks?artist_id=X` 现在同时命中「专辑属于 X」和「歌曲关联了 X」的曲目，
两个条件取并集，不会重复返回同一首。`q` 模糊搜索同样覆盖歌曲关联的艺人名。

**上传限制**：音频 `mp3/flac/wav/m4a/aac/ogg/opus`，单个最大 **1GB**。
`order_index` 缺省追加到末尾；显式指定时在该位置插入，其余歌曲顺序自动后移/压实。

**MD5 去重**：上传接口会在服务端计算文件 MD5 并写入 `Track.file_md5`。
如果同一 MD5 已存在，上传返回 `409`（消息包含已有歌曲 ID）；导入工具应先用
`GET /tracks?md5=<md5>` 查询并复用已有歌曲。`/tracks` 的 `md5` 参数为精确匹配。
历史歌曲若没有 `file_md5`，可运行一次 `python run.py --backfill-md5 --init-only` 补算。

**上传开关**：运行时配置 `allow_user_audio_upload=false` 时，普通用户不能调用
`/tracks/upload` 或 `/files/upload?kind=audio`；`root` / `admin` 不受限。
外部音频地址不受该开关限制，但只允许专辑所有者添加。

---

## 6. 歌词 `/tracks/{id}/lyrics`

歌词以 **LRC 文本**保存，仅接受带时间戳的格式：

```lrc
[ti:潮汐来信]
[ar:星野结衣]
[offset:+500]
[00:01.00][00:02.00]同一句歌词
[00:03.50]下一句
```

| 接口 | 方法 | 权限 | 说明 |
|---|---|---|---|
| `/tracks/{id}/lyrics` | GET | 公开/所有者 | `{track_id, lyrics, updated_at}`；无歌词时 `lyrics` 为空串 |
| `/tracks/{id}/lyrics` | PUT | 歌曲管理 | `{lyrics}`，1–200000 字符；后端校验 LRC |
| `/tracks/{id}/lyrics` | DELETE | 歌曲管理 | 清空歌词 |

校验规则：
- 每一条非空行必须包含 `[mm:ss]` / `[mm:ss.x]` / `[mm:ss.xx]` / `[mm:ss.xxx]` 时间戳，或属于
  `[ti:] [ar:] [al:] [by:] [offset:]` 等元数据标签；
- 至少一行带时间戳；秒数 00–59，分钟最多 3 位；
- 空歌词返回 400；客户端可用 `DELETE` 清除。
- 一行可带多个时间戳，表示同一句在多个时间点出现。

App 可直接解析原始 LRC；歌词面板按时间高亮。后端不提供第三方歌词抓取。

---

## 7. 评论 `/tracks/{id}/comments`

评论挂在**歌曲**上，`parent_id` 为空是主楼，否则是一层回复；回复不能嵌套到第二层。

```json
{
  "id": 7, "track_id": 10, "parent_id": null, "content": "好听",
  "created_at": "2026-10-21T10:00:00", "author": "小明", "author_id": "uuid",
  "author_avatar": "/media/avatars/x.jpg", "source": null,
  "replies": [ { "id": 8, "track_id": 10, "parent_id": 7, "content": "同意", "replies": [] } ]
}
```

| 接口 | 方法 | 权限 | 说明 |
|---|---|---|---|
| `/tracks/{id}/comments` | GET | 公开/所有者 | `page` `page_size` `sort=recommended|time` `seed`；回复随主楼返回 |
| `/tracks/{id}/comments` | POST | 登录 | `{content, parent_id?}`；回复统一挂到主楼 |
| `/comments/{id}` | DELETE | 作者 / 专辑所有者 / 管理员 | 删主楼会级联删除回复 |
| `/tracks/{id}/comments` | DELETE | 歌曲管理 | 清空该歌曲全部评论，返回 `{deleted: n}` |

`recommended` 没有推荐算法：主楼 ID 按 `seed` 随机打乱后分页；同一次刷新翻页需复用同一个 `seed`。

---

## 8. 收藏 `/favorites` 与列表 `/favorite-lists`

收藏针对**歌曲**，且必须归属至少一个用户自建列表；同一首歌可属于多个列表。

### 8.1 收藏列表

| 接口 | 方法 | 说明 |
|---|---|---|
| `/favorite-lists` | GET | 我的列表，含每个列表可见收藏数 `count`；按 `position` 升序（用户自定义顺序） |
| `/favorite-lists` | POST | `{name}`，同名 409；新列表排在最后 |
| `/favorite-lists/order` | PUT | `{list_ids:[...]}` 调整列表顺序；必须包含当前账号全部列表 id，按下标写入 `position` |
| `/favorite-lists/{id}` | PATCH | `{name}` 重命名 |
| `/favorite-lists/{id}` | DELETE | 删除列表；不再属于任何列表的收藏随之删除 |
| `/favorite-lists/{id}/order` | PUT | `{track_ids:[...]}` 调整列表内歌曲顺序；必须包含该列表当前全部可见歌曲 id |

### 8.2 收藏

| 接口 | 方法 | 说明 |
|---|---|---|
| `/favorites` | GET | `page` `page_size` `list_id?`；不传 `list_id` 时按 `collected_at`（空则创建时间）倒序；传 `list_id` 时按 `favorite_items.position` 升序（新加入的歌曲取 `min-1`，始终排第 1 项） |
| `/favorites` | POST | `{track_id, list_ids:[...], collected_at?}`；`list_ids` 至少一个；新加入的歌曲排在该列表第 1 项 |
| `/favorites/{track_id}` | PATCH | `{list_ids?, collected_at?}`；`list_ids` 整体替换；只补列表时不传 `collected_at`，不会改变原列表顺序 |
| `/favorites/{track_id}` | DELETE | 取消收藏 |
| `/favorites/check/{track_id}` | GET | 单曲是否已收藏 |
| `/favorites/check-batch` | GET | `track_ids=1,2,3`（最多 200 个），返回 `FavoriteCheck[]` |

收藏对象：

```json
{ "id": 1, "track_id": 10, "created_at": "...", "collected_at": "...",
  "list_ids": [1, 2], "track": { "id": 10, "title": "...", "album": { "artist": { } } } }
```

`check-batch` 返回：

```json
[ { "track_id": 10, "favorited": true, "list_ids": [1], "collected_at": "..." } ]
```

私有专辑中的歌曲不会出现在其他人的收藏列表/计数/批量检查中。

---

## 9. 播放历史 `/history`

按 `(用户, 歌曲)` 记录进度。

| 接口 | 方法 | 说明 |
|---|---|---|
| `/history` | GET | `page` `page_size`；按 `updated_at` 倒序；每条含 `track`（专辑/艺人） |
| `/history/track/{track_id}` | GET | 某首歌的播放记录，无记录时 `data: null` |
| `/history/album/{album_id}/last` | GET | 该专辑最近一条记录，用于“继续播放” |
| `/history` | POST | `{track_id, progress_sec?, duration_sec?, completed?}`；同一用户+歌曲更新 |
| `/history/{history_id}` | DELETE | 删除一条 |
| `/history/album/{album_id}` | DELETE | 清除某专辑下的历史 |
| `/history` | DELETE | 清空全部历史 |

`completed` 不传时，进度 ≥95% 自动标记完成；上报的实际时长会在歌曲时长为空时补写
`Track.duration_sec`。私有专辑中的历史不会出现在其他人的历史列表。

---

## 10. 文件 `/files`

### 10.1 上传

`POST /files/upload` · 登录 · `multipart/form-data`

| 参数 | 位置 | 说明 |
|---|---|---|
| kind | query | `cover` / `avatar` / `audio` |
| file | form | 文件本体 |

限制：
- 图片 `jpg/jpeg/png/webp/gif/bmp`，≤10MB；
- 音频 `mp3/flac/wav/m4a/aac/ogg/opus`，≤1GB；
- 随机 UUID 文件名，按类型分目录存储；
- `kind=audio` 受 `allow_user_audio_upload` 开关限制。

```json
{ "url": "/media/audio/9f2c....wav", "filename": "9f2c....wav",
  "size": 264644, "content_type": "audio/wav", "kind": "audio",
  "md5": "d83a7748ec0557d843d1903a36b6cc06" }
```

所有上传都会返回 `md5`；歌曲上传接口会用它做重复检测（409）。

---

## 11. 系统

### 11.1 健康检查

`GET /health` · 公开 · **根路径，不在 `/api/v1` 下**

进程存活探针，顺带返回曲库规模，用于部署自检与监控抓取。

```json
{ "code": 0, "message": "ok",
  "data": { "status": "running", "version": "2.0.0", "debug": false,
            "stats": { "artists": 6, "albums": 8, "tracks": 39, "users": 1 } } }
```

| 字段 | 类型 | 说明 |
|---|---|---|
| `status` | string | 固定 `"running"`。请求能被处理就是这个值，不代表数据库之外的东西也正常 |
| `version` | string | 服务端版本，随发布变化（当前 `2.0.0`） |
| `debug` | bool | 是否以 `--debug` 启动 |
| `stats` | object | `artists` / `albums` / `tracks` / `users` 四张表的行数 |

以下行为此前文档没有写清，容易误判：

- **不需要认证**：不带 `Authorization` 也能访问，也不受 root「仅本机」限制；
- **`/api/v1/health` 返回 404**：它和业务接口不是同一个前缀，客户端不要拼 Base URL；
- **数据库读不出来时不报 5xx**：任一统计查询失败，`stats` 退化成 `{}`，`status` 仍是
  `"running"`，HTTP 仍是 200。监控要以 `stats` 是否为空来判断，不能只看状态码；
- 只证明「进程能响应 + 数据库可读」，不检查磁盘空间、媒体目录与外部依赖；
- 每次都实时执行四条 `COUNT(*)`，曲库很大时会有额外开销，探针间隔不要太短。

### 11.2 静态媒体

`GET /media/{path}` · 公开 · **根路径**

封面、头像、音频文件。支持 `Range` 请求（`206 Partial Content`），音频可拖动/断点续传。
私有专辑只限制 JSON 接口可见性；媒体 URL 仍是随机文件名，请勿泄露私有资源链接。

---

## 12. 数据模型速查

| 模型 | 关键字段 |
|---|---|
| User | `id`(uuid) `username` `nickname` `avatar_url` `bio` `role`(user/admin/root) `is_active` |
| Artist | `id` `name`(唯一) `cover_url` `bio` `owner_id` |
| Album | `id` `artist_id` `title` `external_id`(外部唯一标识) `cover_url` `description` `album_type` `release_date` `release_year` `score` `owner_id` `view_count` `is_public`；标签在 `album_tags` |
| Track | `id` `album_id` `title` `order_index` `audio_url` `file_size` `file_md5` `duration_sec` `lyrics`(LRC) |
| TrackArtist | `id` `track_id` `artist_id` `position`（`track_id + artist_id` 唯一）；歌曲的参与艺人 |
| AlbumArtist | `id` `album_id` `artist_id` `position`（`album_id + artist_id` 唯一）；专辑的参与艺人 |
| AlbumTag | `id` `album_id` `name`（`album_id + name` 唯一） |
| Comment | `id` `track_id` `user_id` `parent_id` `content` `author_name` `author_avatar` `source` `source_id` |
| Favorite | `id` `user_id` `track_id`（唯一）`collected_at` |
| FavoriteList | `id` `user_id` `name`（`user_id + name` 唯一） |
| FavoriteItem | `id` `list_id` `favorite_id`（唯一） |
| ListenHistory | `id` `user_id` `track_id`（唯一）`progress_sec` `duration_sec` `completed` |

---

## 13. 实现说明（Rust 版）

- 单二进制部署：`guobeng-music-server`（Windows 为 `.exe`）+ `config.json` + `data/` 即可运行，不依赖 Python。
- 数据库为全新 SQLite（`data/music.db`），不读取旧 Python 库；媒体目录仍为
  `data/media/{covers,avatars,audio}`。
- 首次启动创建 root 超级管理员，随机密码写入 `data/root_password.txt` 并打印到终端；
  root 仅允许本机登录。
- 多艺人：`track_artists` / `album_artists` 是唯一事实来源；新建专辑会把主艺人写入
  `album_artists`，上传歌曲时合作艺人会自动并入专辑艺人。
- 配置文件 `config.json`（启动目录，唯一配置文件）：
  ```json
  {
    "allow_registration": true,
    "allow_user_audio_upload": true,
    "backend_port": 8000,
    "host": "127.0.0.1"
  }
  ```
  - 业务开关（`allow_*`）修改后立即生效；
  - 端口与监听地址只在启动时读取；
    端口优先级：`--port` > `MUSIC_SERVER_PORT` > 本文件 > 8000；
    地址优先级：`--host` > `MUSIC_SERVER_HOST` > 本文件 > `127.0.0.1`；
  - 缺键时按默认值补写回文件，已有值不动。
- 环境变量：`MUSIC_SERVER_HOST/PORT/SECRET_KEY/ADMIN_ONLY_CREATE/CORS_ORIGINS/DEBUG/DATA_DIR/ENABLE_DOCS`。
