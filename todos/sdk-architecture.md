# SDK 架构整理

基线：main 238e57d（SDK 文件传输与 VOD 已合并）。本轮先整理现有实现，直播单独推进。

## 必须保持的行为

- 兼容 `q2q::client::builder()`、发布、下载、恢复、open 和低层 PeerSession API。
- 普通下载仍先耗尽当前 Peer 候选再进入 CDN；VOD 仍使用固定读取窗口和紧急块回退。
- 网络字节在写入前验证；磁盘复核、数据同步、检查点同步成功后才完成租约、增长进度、发送 Have/BlockDone。
- 每块只有一个提交者；取消不释放在途阻塞操作持有的目标锁；磁盘失败阻止后续提交。
- 恢复重新验证磁盘内容，完整文件离线可用；已有文件不会被静默覆盖。
- 源码和测试保持扁平目录；不为整理引入新 crate 或公开内部抽象。

## 执行顺序

- [x] 1. 统一目标存储：Destination 封装共享锁、阻塞 I/O、提交、校验读取和完成同步，Peer/CDN 共用提交入口。
- [x] 2. 区分块队列与下载策略：块队列保留独占租约和完成状态；窗口、来源可用性、期限及来源错误由策略管理。
- [x] 3. 统一下载执行层：Client 委托创建下载，Reader 保留游标适配；Peer 发现与并发 worker 共用实现；CDN 传输与任务编排分开，同时保留两种回退策略。
- [x] 4. 整理协议和会话边界：拆出 Peer 文件协议与内部错误模块；共享控制会话负责活跃句柄心跳维护，取消与错误传播有明确所有者。

## 验收

逐项检查调用边界、保留现有回归覆盖。重点检查并发租约、坏来源、检查点失败、取消时在途磁盘操作、恢复、EOF/seek 和混合 P2P/CDN。完成后运行现有 SDK 与工作区测试、格式和 Clippy，记录实际结果。


## 完成后的模块边界

| 模块 | 职责 |
| --- | --- |
| client / transfer / reader | 构建入口、公开任务句柄、单游标 AsyncRead/AsyncSeek 适配 |
| download | 两种下载模式的初始化、Peer 发现、CDN worker 和后台任务所有权 |
| scheduler | 共用 Peer 连接池、块任务执行，以及来源期限触发的 Peer 中断 |
| block_queue / download_policy | 块租约与完成通知 / 来源和读取窗口策略；在同一个队列锁下原子更新 |
| destination / download_store | 异步磁盘边界 / 同步文件、排他锁、校验和检查点格式 |
| peer_transfer / cdn | Peer 文件协议会话 / HTTP Range、叶子表和镜像取块 |
| publication / source_file | 发布和撤销生命周期 / 本地文件准备及上传前复核 |
| control / online | 共享控制会话 / 由活跃句柄持有的单个心跳任务 |
| error / task_scope / progress | 类型化错误、在途任务计数、合并进度通知 |

没有新增 crate 或目录层级。队列通过策略对象作选择，策略不运行网络任务；PeerDownload 只处理取块、确认和结束，scheduler 在统一提交成功后调用确认。整文件模式保留“当前镜像的 worker 退出后再换镜像”，VOD 保留“当前紧急块回退且其他 Peer 继续”的行为。

存储裸锁与 DownloadStore 的 Deref/DerefMut 只对私有故障注入测试开放；生产路径通过 Destination 的窄接口访问。测试辅助存储入口也移到 tests/destination.rs，公开 Transfer 模块只保留句柄与进度类型重导出。

### 错误兼容说明

构建、发布、下载、恢复、open 和低层 PeerSession 的调用签名及原有类型路径不变。ControlError 新增 Shared(Arc<ControlError>)，用于向多个活跃句柄报告同一个心跳失败，并通过 Error::source 保留原始原因；调用方若穷举匹配 ControlError，需要增加 Shared 分支。读取队列也保留 Arc<TransferError>，到 AsyncRead 边界才转换为 io::Error。

## 验证记录

- 工作区全部 feature：272 项通过，2 项外部 Redis 测试按原配置忽略。
- 最终 SDK 53 项（43 单元 + 10 公开 API 集成）通过；共享心跳新增去重/原始失败共享、末个句柄释放、异常退出传播测试。
- 格式、全工作区 all-target/all-feature Clippy（warnings denied）、diff 检查通过。
- 本地真实 Track/Punch/WebRTC：随机 VOD / 跨块读取、健康 P2P 零 HTTP、源文件变更后仅请求所需 CDN 块，通过。
- 独立本地 Track：两发布者并行下载 16,780,339 字节，活跃 Peer 峰值为 2，损坏块修复及离线完成恢复，通过。
- 短租约联机：两个客户端各自同时发布和按需读取，使用 12 秒租约、2 秒心跳，等待 15 秒跨过多次心跳及文件续租/密钥轮换后继续读取并验证完整内容，通过。
- 首次复用旧本地 Track 时遇到测试速率限制，隔离到独立内存 Track 后上述并行用例通过；测试不使用外部 Redis。


## 并发上传后的结构收敛

- [x] 接入所有权：admission.rs 提供托管独占/低层并发接入租约，冲突明确返回错误，低层 API 不再无条件改变托管生命周期。
- [x] 暂停先发布独立通知，再获取接入队列锁；请求等待者和锁等待者都能取消等待，避免关闭死锁。
- [x] upload.rs 保留上传池编排；单连接上下文、事件消费、上传状态机与发送移入 upload_session.rs，消除文件协议对上传池上下文的依赖。
- [x] publication.rs 提取私有运行器与续租函数，启动失败、停止、错误统一进入同一清理路径，保留原始错误。
- [x] 在接入通知顺序、租约持有范围、额度释放、事件消费和终态发布边界补充注释。
- [x] 上传专项测试移到扁平 tests/upload_session.rs，恢复子进程测试的原路径保持不变。

不新增 crate，不引入通用传输 trait、插件层或目录层级。接下来先交付并发上传 PR；热点缓存、并发随机读取按性能数据单独评估。

### 本轮验证

- 工作区 all-features：305 项通过，SDK 84 项；Redis 集成测试未重跑。
- 本地 Track/Punch 两次通过：并发/限速/公平/统计、关闭与源变化、两种接入所有权冲突，以及取消低层等待后成功重新发布。
- 全工作区 Clippy（all-targets/all-features、拒绝警告）、fmt、diff 检查和中英文文档构建通过；transport 架构图和 wire 页浏览器检查通过。


源读取后续改为共享只读 PreparedFile、独立读取额度及按偏移 I/O，见 [positional-source-reads.md](./positional-source-reads.md)。上传协议继续通过 UploadContext 维护阻塞任务生命周期；上传池不承担磁盘调度。
