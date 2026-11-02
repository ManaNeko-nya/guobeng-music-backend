# Rust 重写与仓库操作笔记

本项目把一个 Python / FastAPI 音乐后端整体重写成 Rust（axum + sqlx + SQLite），
产出三平台二进制，单个 3.4–3.6 MB，无运行时依赖。下面是这次真正踩到的东西，
按「编译环境 → 跨平台 → 代码组织 → 仓库与 CI」排列，能直接抄的就抄。

---

## 一、编译环境

### 1.1 Windows 上 MSVC 与 MinGW 的区别比想象中深

`rustup` 默认装的是 `stable-x86_64-pc-windows-msvc`，它要 VS Build Tools 里的 `link.exe`。
没装时的失败点很反直觉：**报错发生在编译依赖的 build script 阶段**，看起来跟自己的代码毫无关系，
很容易误以为 crate 版本不兼容。

只给 cargo 加 `--target x86_64-pc-windows-gnu` 解决不了：**target 只改产物，不改 host**。
build script 仍由 host 工具链编译，仍然要 MSVC 的链接器。

要让整条链路都不碰 MSVC，必须换默认工具链：

```powershell
rustup toolchain install stable-x86_64-pc-windows-gnu
rustup default stable-x86_64-pc-windows-gnu
rustc -vV          # host 应为 x86_64-pc-windows-gnu
```

换完之后还有一个坑：gnu 目标下 `windows-sys`、`parking_lot_core` 这类带 `.def` 的 crate
需要 `dlltool.exe`。MinGW 的 `bin` 不在 PATH 就会失败，而报错文案像 crate 本身的问题。
把 MinGW 的 `bin` 目录加进 PATH 即可。

### 1.2 一个 feature 能制造上百个编译错误

`sqlx` 用 `default-features = false` 时，`FromRow` 的 derive 宏在单独的 `derive` feature 里。
漏掉它，编译会吐 126 条 `the trait bound ... is not satisfied`，全部指向 `query_as`，
根因完全看不出来。

**手动关掉默认 feature 后，逐个核对用到的宏 / derive 是否还开着。**

### 1.3 E0716：临时值被借用

```rust
// 错：format! 产生的临时 String 在语句结束时被丢弃，借用无效
sqlx::query_as::<_, Row>(&format!("SELECT ... WHERE id = ?"))

// 对：先绑定，再借用
let sql = format!("SELECT ... WHERE id = ?");
sqlx::query_as::<_, Row>(&sql)
```

### 1.4 debug 慢 25 倍，通常不是你的代码

登录接口 debug 7.9 秒、release 0.31 秒。根因是 PBKDF2 的 60 万轮迭代跑在**未优化的依赖**里。

```toml
[profile.dev.package."*"]
opt-level = 3
```

这行只优化依赖，自己的 crate 仍保留完整 debug 信息与增量编译。7.9s → 2.2s。
代价是首次编译变慢、`target/debug` 变大；只要在开发期调密码哈希、压缩、加解密这类重计算，就非常值。

### 1.5 release 目录可能是"脏"的

用 MSVC 编过之后切到 gnu，`target/release` 里残留的半成品会让链接报 `os error 5`
（拒绝访问），看不出跟工具链有关。

**切换工具链、改 `RUSTFLAGS` 之后遇到莫名其妙的链接错误，先 `cargo clean --release`。**

---

## 二、跨平台与静态产物

### 2.1 目标怎么选

| 平台 | target | 关键开关 | 理由 |
|---|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-musl` | — | 完全静态，不依赖目标机 glibc |
| Linux arm64 | `aarch64-unknown-linux-musl` | — | 同上，交叉编译产物 |
| Windows x86_64 | `x86_64-pc-windows-gnu` | `RUSTFLAGS=-C target-feature=+crt-static` | 不依赖 `VCRUNTIME140.dll` |

### 2.2 为什么不发 gnu 动态链接

glibc 动态产物要求目标机有匹配的 `ld-linux`。在 Ubuntu 22.04 上编好丢到 Ubuntu 20.04，
报的是 `cannot execute: required file not found`——**这句说的是加载器找不到，不是程序找不到文件**，
很容易误判成缺文件。

Termux 上更是彻底不行：Android 用的是 bionic，系统里根本没有 glibc。
所以"给 Termux / 任意 Linux 用"和"gnu 动态链接"是互斥的，只能静态。

### 2.3 交叉编译别自己搭 sysroot

一开始按常规做法装 `gcc-aarch64-linux-gnu` + `libc6-dev-arm64-cross`，结果编 `sqlite3.c` 时报
`bits/libc-header-start.h: No such file or directory`。根因是交叉 gcc 没真正用上交叉头文件，
退回去读了宿主机的 `/usr/include`。

改用 **musl 目标 + `cross` 官方镜像**后一次通过。镜像里的 musl 交叉工具链是配好且被 cross 使用的，
`libsqlite3-sys`、`ring` 这些要编 C 代码的依赖不再需要手工搭 sysroot。

如果坚持自己搭，至少在 CI 里加一条显式校验，把"装了但没被用上"提前暴露：

```bash
test -f /usr/aarch64-linux-gnu/include/stdio.h || { echo "交叉头文件缺失"; exit 1; }
```

### 2.4 静态校验的正则要放宽

同一个 musl 目标，不同架构 `file` 的输出不一样：

- `x86_64-unknown-linux-musl` → `static-pie linked`（带 ASLR）
- `aarch64-unknown-linux-musl` → `statically linked`

只匹配 `statically linked` 会把 x86_64 判成失败：

```bash
file "$BIN" | grep -qE "statically linked|static-pie linked"
```

---

## 三、代码组织

- **事务边界放路由层**：service 只改对象、不 commit。谁开事务谁负责回滚，半径清晰，
  也不会出现"service 里顺手 commit 导致外层回滚不掉"。
- **统一响应信封**：`{code, message, data}`，`code` 直接用 HTTP 状态码、成功为 0。
  前端只判一个字段，不用同时看状态码和 body。
- **422 要带回字段名**：从 serde 的报错里抠出真实字段，返回
  `data.errors: [{field, message}]`，前端可以直接映射到表单控件。
- **配置补写只补不删**：`config.json` 缺键时写入默认值；不删除多余键，也不写入本次的覆盖值。
  否则用户会以为某个选项改了有效，其实根本没被读过。

---

## 四、仓库与 CI

### 4.1 gh CLI 用 device flow

```bash
gh auth login --hostname github.com --git-protocol https --web
```

需要的 scope：`repo`、**`workflow`**、`delete_repo`（重建仓库时才用）。

`workflow` 不能漏：推送 `.github/workflows/*` 时会被拒，
报错是 "refusing to allow an OAuth App to create or update workflow"，**不会提示你缺 scope**。

### 4.2 手动触发与看产物

```bash
gh workflow run build.yml --ref main
gh run watch <run-id> --interval 20 --exit-status

# 产物清单（需要认证，浏览器直接开链接只能看到页面）
gh api repos/<owner>/<repo>/actions/runs/<id>/artifacts \
  --jq '.artifacts[]|{name, mb:(.size_in_bytes/1048576*100|round/100)}'

# 某个 run 的告警条数，用于确认弃用警告是否消失
gh api repos/<owner>/<repo>/actions/runs/<id>/annotations --jq 'length'
```

### 4.3 只通 api.github.com 时怎么推

现象：`github.com` 走代理 502、直连超时，`api.github.com` 正常。
结果 `git push` / `git fetch` 全废，但仓库管理、触发构建、读日志都能用。

应急做法是 Git Data API：

1. `POST /repos/{o}/{r}/git/blobs`（`content` + `encoding=base64`）逐文件上传
2. `POST /git/trees`（`base_tree` 指父提交的 tree，`tree` 只列改动文件）
3. `POST /git/commits`（`message` + `tree` + `parents`）
4. `PATCH /git/refs/heads/main`（`sha` + `force`）

**副作用（踩过）**：远端生成的 commit SHA 与本地不同，下次 `git push` 直接非快进。
两个根因，缺一不可：

- GitHub 把提交时间统一转 UTC 存储，本地时区偏移无法逐字节复现；
- 它存的 commit 原文里 **message 末尾没有换行**，而本地 `git commit` 会带。

对齐办法：从 API 拿到 commit 原文，本地复算哈希，一致就写回对象库并挪引用。

```bash
git hash-object -t commit --stdin < commit_obj.txt        # 与远端 sha 比对
git hash-object -w -t commit --stdin < commit_obj.txt     # 一致则写入本地对象库
git update-ref refs/heads/main <sha>
git update-ref refs/remotes/origin/main <sha>
```

对齐后 `git status -sb` 显示 `## main...origin/main`，无 ahead/behind，以后正常 push 即可，
不需要 `--force`。

**结论：API 推送只当应急。能用 git 就别用；用了必须事后对齐，否则每次 push 都是麻烦。**

### 4.4 Actions 的运行时版本

- `actions/checkout@v4` 跑 Node 20，已被强制转 Node 24 并告警 → 升 `v5`（本身就是 node24）。
- `actions/upload-artifact@v5` **仍声明 `node20`**，升到 v5 警告不会消失；`v6` 起才真正用 node24，
  现在用 `v7`（新增可选 `archive` 参数，`name` / `path` / `if-no-files-found` 全部兼容）。

判断某个 action 实际跑在哪个运行时，直接读它的 `action.yml`，别猜：

```bash
gh api repos/actions/upload-artifact/contents/action.yml?ref=v7.0.2 \
  --jq '.content' | base64 -d | grep -A3 '^runs:'
```

### 4.5 私有仓库没有免费的 arm64 runner

`runs-on: ubuntu-24.04-arm` 在私有仓库里拿不到机器（排队或直接失败）。
只能用 `ubuntu-22.04` + `cross` 交叉编译两个架构——这也是前面选 musl 的直接原因之一。

本仓库现已公开，公开仓库可以申请到免费的 arm64 runner；但工作流仍保留 `cross`，
因为交叉编译在两个架构上共用同一条构建路径，比多维护一份原生 job 更省事。

### 4.6 改名要连 Cargo.lock 一起改

CI 用了 `--locked`。只改 `Cargo.toml` 的 `name` 而不同步 `Cargo.lock`，构建在第一步就失败。
不触发编译的校验方式：

```bash
cargo metadata --locked --offline --no-deps --format-version 1 >/dev/null
```

---

## 五、清单

发布前：

- [ ] `cargo clippy --all-targets -- -D warnings` 干净
- [ ] `cargo build --release --locked` 通过
- [ ] 改过包名 / 依赖后 `cargo metadata --locked --offline --no-deps` 通过
- [ ] 产物静态校验（`file` + `statically linked|static-pie linked`）
- [ ] 冒烟覆盖：增删改查、上传、多值关联、Range、权限

推仓库前：

- [ ] `git status -sb` 无 ahead / behind
- [ ] 近期用过 API 推送的话，确认本地与远端 SHA 一致
- [ ] 触发一次 CI，确认注解数为 0

---

## 六、回头看

1. **报错在依赖的 build script 里，八成是工具链，不是代码。** 先 `rustc -vV` 看 host，再怀疑 crate。
2. **性能数字要实测。** 7.9s → 0.31s 是量出来的；没有实测就不写"更快"。
3. **应急手段要留退路。** API 推送救过一次，代价是 SHA 分叉，事后补了对齐。
4. **静态检查写宽一点。** `static-pie` 这种变体会让"严格"的正则误杀，白白卡住发布。
5. **默认配置要落地到文件。** 只在代码里设默认值，用户永远不知道自己能不能改端口。
