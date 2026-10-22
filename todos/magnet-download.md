# Q2Q 磁力链下载方案

状态：磁力链下载核心已完成实现与本地验证；Platform 磁力链展示已接入。远端 CI 状态以 PR 检查为准，尚未发布。

## 目标与现状

支持 `q2q download 'magnet:?xt=urn:sha256:…'`：从 Track 找 Peer，经已认证的 Peer 控制通道取得并验证 info，然后复用现有下载、Merkle 验证、检查点和恢复流程。SDK 提供同样能力供桌面端使用。

实施前已有 `Magnet` 编解码、按 info hash 查询 Track 和建立 Peer 会话。现已补齐元数据消息、上传端响应、`Client::resolve_magnet`、校验缓存及 CLI 接线；既有下载 API 继续使用 `Seed`。

依据：[协议 4.3、5.1–5.3](../docs/zh/spec.md)、[Peer wire](../crates/q2q-primitives/src/peer_wire.rs)、[core 下载](../crates/q2q-core/src/download.rs)、[CLI](../crates/q2q/src/app.rs)。

## 第一阶段边界

- 使用 Q2Q 的 `urn:sha256`，不实现 BitTorrent 的 `btih`/`btmh`。
- Track 继续只承担发现和控制面职责，不存储或返回 seed/info；Punch 不增加元数据存储。
- 先实现从在线 Peer 获取 info 的 P2P 下载。完整持有者和部分持有者都可以提供 info。
- `cdn_list`、`proof_list`、`leaf_hashes` 属于 seed 外层，不能由 info hash 推导。第一阶段取得 info 后构造没有 CDN 提示的最小 seed，内容证明由 Peer 提供。
- 因此，仅有 CDN、没有在线 Peer 的文件，无法靠磁力链冷启动。此前通过 CDN 下载的照片不能仅凭本轮下载成功就断言其磁力链可用，需另外验证是否有在线 Peer。
- 暂不加入 `xs`/`as` HTTP 元数据源或系统 scheme 注册。平台已在已发布版本中展示磁力链；完整 `.q2q` 文件保留其 CDN 冷启动能力。

## 信任与元数据表示

`xt` 中的 info hash 是内容身份；`dn` 与 `tr` 都是提示。显示名不能替代已验证的 `info.name`，Track 地址只用于寻找来源。

接收流程：先检查控制消息和 info 字节上限，再验证 `SHA-256(canonical info bytes) == xt`，解析字段并检查版本、长度、块大小和 core 当前块数上限。全部通过后才允许创建下载文件、分配位图或缓存 metadata。

复用现有 canonical bencode 规则，拒绝非 canonical 编码。必须处理当前 `Info::from_value` 丢弃未知字段后重新编码会改变 hash 的问题：解析值与原始身份应绑定，保留未知字段的 canonical 表示，不能把仅对已知字段重新计算的 hash 当成原始 hash。该调整同时覆盖 seed 与 magnet 路径，并评估公开 `Info` 构造方式的兼容性。

建议 info 原始字节上限为 8 KiB，编码为 base64 后连同 JSON envelope 仍需满足当前 16 KiB 控制消息上限。name/mime 可变长，不能因 info 不携带叶子列表就假设其永远很小。超限内容仍可使用完整 seed，磁力链元数据服务返回明确限制错误。

## Peer 协议与状态

在 `q2q-primitives::peer_wire::Control` 增加：

```text
RequestInfo
Info { data: <canonical info bytes 的 base64> }
```

连接已通过内容身份绑定，一条会话对应一个 info hash，消息不重复携带 hash。

下载端：认证建连 → 发 RequestInfo → 收 Info → 校验 → 关闭元数据连接 → 进入现有下载流程。

上传端：认证建连 → 等待首条 RequestInfo 或 Bitfield；收到 RequestInfo 时返回本内容的 info，随后等待 Bitfield；收到 Bitfield 时按现有流程传输。info 就绪前不发送数据请求。每连接只允许一次元数据请求，限定等待时间；对重复请求、错误顺序或超限载荷结束该连接，错误不传播为整个发布失败。

这是现有控制协议的增量消息：新上传端继续接受旧客户端直接发送 Bitfield。旧上传端无法识别 RequestInfo 时，磁力链解析把该 Peer 视为不支持并尝试其他候选；既有 seed 下载流程不要求新增消息。第一阶段不新增能力协商往返。

上传端只发送本会话内容的 info；复用既有上传连接额度、握手截止时间和取消清理，不另建无界元数据服务任务。

## Core API 与职责

已增加 `Client::resolve_magnet(&Magnet) -> Result<Seed, MetadataError>`，先返回经验证的最小 seed，让 CLI 和桌面端确定安全的目标路径后再调用现有 download/resume API。另提供不构建 Client 即可读取并重验缓存的 `client::cached_magnet`。

- 当前 Client 使用其已配置的 Track，不在 SDK 写死官方地址；CLI 复用现有 Track failover，对各候选 Client 调用解析 API。
- core 内复用 Track query、Peer 认证建连与候选过滤逻辑。元数据请求最多两个并发连接，且不超过已有客户端连接限制；发现候选保持现有查询上限，查询和重试受总截止时间约束。
- 建议整轮解析截止时间 60 秒，连接沿用当前上限并受剩余总预算限制，建连后等 info 最多 10 秒。先作为内部常量，不增加 CLI 参数。
- 某个 Peer 返回错误 hash、畸形 info 或超限内容，只淘汰该候选并继续；一个合法结果即成功，并取消、等待清理其余任务。候选耗尽返回有上下文的错误，不无限等待。
- 第一阶段成功后关闭元数据连接，再复用现有下载入口建连。这样 resolve 的返回值没有隐藏会话所有权；后续有实测必要再优化为复用连接。
- 调用 future 被取消时关闭元数据连接及其控制面资源；客户端整体连接的生命周期遵循现有 SDK 约定。

## Metadata 缓存与恢复

缓存由 core 维护，使用其 `storage_dir` 下的 `metadata/<info_hash>.q2q`；CLI 中自然位于 `Q2Q_HOME` 内。缓存保存经验证的最小 seed，不持久化磁力链提示中的显示名或可能携带凭据的 Track URL。

使用临时文件加原子替换写入；读取时检查大小、重新解析并比对期望 info hash。缓存损坏视为 miss 后重新发现，权限/磁盘错误保留具体原因。缓存只减少 metadata 网络请求，不代替块与检查点校验。

重复执行相同磁力链时，先取有效缓存，再走现有检查点恢复。已有完整目标可沿用现有离线完成检查；未完成文件仍需要可用内容来源。失败或取消元数据阶段不创建空目标或下载检查点。

## CLI 行为

```sh
q2q download 'magnet:?xt=urn:sha256:…'
q2q download 'magnet:?xt=urn:sha256:…' --output ./photo.jpeg
q2q magnet ./photo.jpeg.q2q
```

`download` 的输入从只含 PathBuf 改为内部 `SeedPath | Magnet`。只有 `magnet:` 前缀才走链接解析；语法错误立即报告，不把错误磁力链当成本地文件路径。

Track 候选顺序：显式 `--track`/环境变量 → 磁力链 `tr` → 官方兜底；沿用已有 URL 校验、去重和 failover，不添加另一套 Track 参数。

未指定 `--output` 时，解析完成后复用现有安全文件名规则选择当前目录目标。没有可用 `info.name` 时使用完整 info hash 作为文件名；不使用未验证的 `dn` 创建路径。显式 output 也须等 info 验证后才创建。

增加轻量的 `magnet <SEED>` 子命令生成分享链接，只读取本地 seed；stdout 输出一行链接，其他信息走 stderr。publish 的默认 stdout 继续输出 seed 路径，避免破坏已有脚本。

元数据阶段在 stderr 显示“查找来源 / 获取元数据”，大小未确定时使用 spinner。`--quiet` 同样生效。下载成功后 stdout 沿用目标路径输出；SIGINT/SIGTERM 延续已有退出码和资源清理语义。

错误区分：链接非法/不支持的 namespace、Track 访问失败、无候选、Peer 不支持、获取超时、metadata 校验失败、超出本地资源限制。聚合失败信息保留原因，日志不输出 URL 的凭据/query。

## 实现顺序与验收

1. [x] **Primitives 与规范**：补齐 info 独立编码/校验及身份保持；加固 magnet 多 xt、冲突 xt、非 ASCII hash、错误 percent 编码和过长输入的处理；冻结消息与限制。验收：畸形链接不 panic；未知字段不改变内容身份；非法 info 不进入后续流程。
2. [x] **Wire 与上传端**：新增 RequestInfo/Info，调整首消息状态机。验收：完整/部分发布者可提供 info；旧 seed 下载继续工作；错误或取消一个元数据连接不影响其他下载方。
3. [x] **Core resolver**：候选发现、受限竞速、验证、错误分类和清理。验收：恶意 Peer 与健康 Peer 混合时可成功；无 Peer、旧 Peer、超时、取消都有界退出；资源无残留。
4. [x] **Metadata 缓存**：原子落盘、读取重验、恢复接线。验收：缓存命中不重新请求 metadata；损坏缓存重新发现；取消解析不留下目标或检查点。
5. [x] **CLI**：输入分流、Track failover、输出名、解析进度和 magnet 子命令。验收：带/不带 tr 的链接、显式 Track、默认官方兜底、output/quiet、空文件、文件名攻击、已有目标与中断恢复。
6. [x] **联机回归与文档**：使用临时本地 Track/Punch 完成“publish → 生成 magnet → 全新身份下载 → 内容一致”闭环，并测试 partial Peer、一个坏 metadata 源及取消。同步中英文 CLI/wire/SDK 文档，移除对应未实现描述；确认默认 `.q2q`、CDN fallback、并发下载和恢复无回归。真实官方服务作为补充验证，不取代可复现的本地场景。

## 验证记录

已从远端已合入的 main 建立 `codex/magnet-download` 分支，并保留本方案文件。

- `cargo test --locked --offline`：工作区 419 项通过、13 项显式忽略；这些忽略项包含依赖外部服务的场景，不视为通过。
- 随后增加并单独运行 CLI 元数据阶段中断场景；最终 `cargo test --locked --offline -p q2q --test cli` 为 11 项通过、2 项显式忽略。
- `cargo clippy --locked --offline --all-targets -- -D warnings`、最终 CLI all-targets Clippy、`cargo fmt --all -- --check` 和 diff 空白检查通过。
- `cargo test --locked --offline -p q2q-primitives --no-default-features --test seed_builder`：3 项通过。
- `cargo clippy --locked --offline -p q2q-wasm --target wasm32-unknown-unknown --release -- -D warnings` 通过。
- `npm run docs:build` 通过，中英文 CLI、wire、SDK 文档及协议说明已同步。
- 使用全新临时本地 Track/Punch，显式运行 core 的 `local_magnet_bad_peer_full_download_cache_and_partial_metadata`：坏 info 被拒绝、坏/健康来源混合后成功解析、实际内容下载一致、部分持有者提供 metadata、缓存命中、完整文件离线恢复、无来源错误均通过。
- 使用另一组全新临时 Track/Punch，显式运行 CLI 的 `magnet_publish_download_interrupt_and_offline_resume`：发布、链接生成、失败 Track 切换、带/不带 tr、中断下载、缓存续传及发布者退出后的离线完成均通过。
- 元数据对错 hash、超限、错误 base64、旧 Peer 首消息、超时、取消和缓存损坏有本地回归；CLI SIGINT 期间目标与检查点均不创建。临时服务已停止。

本轮在 Linux 上执行；没有把 Windows/macOS、线上服务或远端 CI 写作已验证。第一阶段 CDN-only 冷启动限制保持不变。

## 实施中补齐的兼容性与修复

- `Info` 保留私有未知字段，既有 hash 对已知字段的结果不变；依赖公开结构体字面量的外部代码需改用 `Info::new` 后设置字段，已在 SDK 文档说明。未知字段参与完整身份，旧实现曾丢字段而产生的错误身份不予延续。
- `request_info` 使用严格空结构消息，拒绝额外字段；wire JSON 形态仍为 `{"type":"request_info"}`，envelope 版本仍为 1。
- 修复默认相对目标路径恢复时，清理旧检查点临时文件对空父路径调用 `read_dir` 导致失败的问题；已有 CLI 恢复测试已覆盖。
- Track HTTP 错误保留错误链，但移除 URL，避免日志带出 query/凭据。
- 部分持有者遵循原有宣告规则：至少有一个已提交且通过校验的块后才进入 Track；空的下载目标不会仅因持有 info 就被宣告为可用来源。

## Track 提示简写

- [x] `tr` 无协议默认 HTTPS，生成省略 HTTPS，显式 HTTP 保留，兼容旧完整 URL。
- [x] 百分号编码/解码一次，过滤无效、非 HTTP(S) 及含凭据的地址。
- [x] 中英文 CLI 与协议说明同步。未运行测试。
- `cargo check --offline -p q2q`、格式及 diff 检查通过。

## Platform 磁力链展示

- [x] 已发布版本显示可复制的磁力链，链接使用固定的 `info_hash`，不包含可选的 `dn` 文件名提示；自定义网络附上该版本 Track，平台网络使用客户端默认 Track。
- [x] 页面说明磁力链不含平台存储地址、不使用下载种子授权，需在线 Peer 提供内容；`.q2q` 下载种子继续用于平台存储冷启动。
- 本轮仅做代码与静态差异审阅，未运行测试或构建。
