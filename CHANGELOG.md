# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构，
版本号使用 [语义化版本](https://semver.org/lang/zh-CN/)。

## [2.3.0] - 2026-11-17

### Changed

- `/health` 公开响应只返回 `{ "status": "running" }`；`version`、`debug`、`stats`
  仅在携带 `admin` / `root` 令牌或服务以 `--debug` 启动时返回。
- `root` 账户不再限制登录来源，与普通账户一样可从任意地址登录、刷新令牌；
  README 与 API 文档明确建议另建 `admin` 账户日常使用。

### Removed

- 移除 2.2.0 引入的 `albums.external_id` 启动迁移程序（原计划在下一版本删除）。
  仍停留在 2.2.0 之前的数据库，请先运行一次 2.2.1 完成迁移，再升级到 2.3.0。

## [2.2.1] - 2026-11-14

### Fixed

- 修复旧库迁移 `albums.external_id` 时只删除唯一索引、仍被普通索引引用而失败的问题；
  现在从 `sqlite_master` 查出所有引用该列的索引后逐个删除，再执行 `DROP COLUMN`。

## [2.2.0] - 2026-11-14

### Added

- 专辑封面与简介可编辑；客户端可从酷狗搜索专辑封面。
- 专辑简介支持写入 `kugouid<ID>` 形式的外部来源标记，导入工具据此查重。

### Removed

- 移除 `albums.external_id` 列、`/albums/by-external/{id}` 接口与相关 DTO 字段；
  旧库启动时自动把该列写入空简介后删列。

## [2.1.0] - 2026-11-14

### Added

- 新增 `POST /tracks/{id}/audio` 更换音源：上传新音频并原子替换 URL、大小、MD5 与时长。

## 更早版本

2.1.0 之前未维护 CHANGELOG。
