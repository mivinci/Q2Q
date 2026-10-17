# 上传统计与进度订阅

## 接口

- `Publication::progress() -> UploadProgress`：即时读取内存快照。
- `Publication::subscribe_progress() -> watch::Receiver<UploadProgress>`：每 100 ms 合并普通通知，新订阅同步最新值，关闭及终态立即通知。
- 类型从 `q2q::publication` 与 `q2q::client` 导出：`PublicationState`、`UploadProgress`、`UploadConnectionProgress`、`UploadConnectionState`。

## 统计边界

- 发布级汇总：握手、活跃、空闲连接数；累计编码发送字节及重传子集；完成、失败、取消连接数。
- 活跃明细：连接 ID、Peer ID、状态、发送/重传字节。只在取得上传额度后登记，连接结束移除，容量拒绝不计入已接入连接统计；不存储无限历史。
- 握手中的 Peer ID 来自已验证 Connect 票据，握手后完成对端认证。
- `Sending` 包含源读取、限速等待、发送和等待 BlockDone；认证成功且无待确认块时为 Idle。
- 只在 DataChannel 成功接受发送后累计编码 piece 字节（含 17 字节头和末尾短片）；重传同时增加总字节与重传子集。不是唯一内容字节、接收确认或持久化进度；控制消息和传输层开销不计。
- 正常结束本次会话才计完成，不等于整文件交付。远端断开、握手/传输/源错误计失败，本地取消计取消；单连接远端错误不影响发布状态。
- 关闭与错误触发 Closing，等待全部 worker 和在途阻塞读取、撤销宣告后进入 Closed/Failed；异步任务异常退出用 RAII 标记失败，连接守卫补齐取消统计。
- 重新发布从零统计；低层调用者自行管理的 serve_file 不计入；速度由应用按时间和计数差计算。

## 实现

`upload_progress.rs` 独立于信令、网络和磁盘。逐片只更新短锁下的计数，不克隆完整快照或唤醒 UI；发布任务复用一个定时 future 刷新 watch，没有新增独立后台任务。worker 持有连接 RAII 守卫，结束/失败显式结算，取消或未开始就被 abort 时自动清理，避免重复计数。

watch 不要求订阅者及时消费通知，允许合并；应用须及时 clone 读到的快照，不得跨 await 持有 watch 借用锁。close 消费句柄前可保留 receiver，之后读取最终快照；Drop 同样触发异步关闭。

## 验证

- [x] 生命周期分区、结束记录移除与有限内存历史。
- [x] 并发计数、重传、取消、重复清理不重复计数。
- [x] 每 100 ms 通知合并；无订阅者、慢订阅者、新订阅、关闭立即通知；异常退出失败与清理通知。
- [x] 真实发送中重传计费；真正失败的 DataChannel send 不计费。
- [x] 本地 Track/Punch：两下载方、远端失败隔离、Idle VOD、close 最终计数、重新发布归零、源变化失败及 Drop 最终通知。
- [x] 全工作区测试、Clippy、fmt、文档构建与浏览器检查。

## 验证结果

- 全工作区 302 项测试通过（SDK 81 项）；Redis 集成测试未重跑。
- 本地 Track/Punch 联机两次通过，覆盖真实发送字节、并发/公平限速、远端失败隔离、关闭终态、重新发布归零、源变化及 Drop。
- 上传清理测试连续 20 次通过；修复了阻塞读取守卫先通知完成、后释放额度的字段析构顺序竞态。
- 连接结束守卫支持幂等结算；连接 ID 复用时，旧守卫不会删除新记录。未开始执行就取消的 worker 也会移除明细并增加取消数。
- Clippy（全部 targets/features、拒绝警告）、fmt、diff 检查、中英文文档构建及 transport 页面浏览器检查通过。
