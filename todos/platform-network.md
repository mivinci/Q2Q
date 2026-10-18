# 项目分发网络

- 项目级 `/network` 页面：平台托管（默认）和自定义 Track 两种模式；与存储分开。
- 托管 Track 从平台环境配置读取，未配置则明确显示不可用；套餐、额度、认证隔离尚未实现，不展示模拟额度或承诺服务。
- 自定义支持一个 HTTP(S) Track 基地址，禁止用户信息、query、fragment；生产使用 HTTPS，开发允许 loopback HTTP。
- 检查连接由浏览器直接读取 Track 的只读 `/health`，不让页面服务端代理用户指定地址。只验证服务身份与连通性，不代表 Punch/TURN 可用；老 Track 缺少 health/CORS 时允许保存并明确提示检查受限。
- 网络配置仅 owner 可保存。发布版本原子读取并固定模式和 Track 地址，历史版本不随项目配置变化。
- 导出授权种子时通过 Wasm 写入该版本的 track_list；基础处理产物保持不变。旧版本无配置则保留未配置，不回填。
- 文件发布区显示当前项目网络，种子详情显示发布时快照；解析空值使用明确文案。

## 进度

- [x] 配置表、发布快照、权限及导出接线
- [x] 分发网络页面、只读健康检查、文件及种子展示
- [x] Track/Wasm Rust 编译、平台类型检查通过，零错误/警告
- [x] 生产构建与消费者 Wrangler dry run 通过
- [ ] 配置保存、发布快照不变性、health/CORS 与导出行为验证（本轮未执行）

新 migration 0014 已应用本地数据库，没有给已有测试版本回填 Track。基础处理产物不改变，授权种子通过 seed_with_network 装配新版本的网络快照。健康接口返回的 service 字段仅用于诊断，不代表服务可信或托管套餐可用。

本轮已查看平台托管和自定义网络两种界面，没有保存测试地址、请求外部 Track 或新建发布/授权。真实配置保存、health/CORS、快照和导出行为待后续验证；现有回归数据库 fixture 已同步 migration 0014，未在本轮运行行为测试。

## 本地 Track / Punch 联调

- 用户随后授权启动本地 Track/Punch 并配置测试。已使用持久化测试证书启动回环 Punch，Track 下发对应公钥；页面保存自定义回环 Track，浏览器 health/CORS 检查通过。
- 修复 Wrangler 本地生产构建 dev=false 时错误拒绝 loopback HTTP：开发模式或 AUTH_ORIGIN 为本机 HTTP 时允许 loopback Track，其他平台仍要求 HTTPS。URL 规范化提取成无环境依赖函数，新增凭据、query、fragment 和非本机 HTTP 拒绝回归。
- 两个独立 CLI/SDK 使用本地 Track/Punch 完成 9 MiB P2P 下载，HTTP 请求为 0，cmp 和 SHA-256 一致；Wasm 网络装配函数确认 Track 地址写入且 info_hash 不变。旧发布版本的网络字段仍为 unconfigured/null，未回填。
- 联调暴露默认 Track 心跳 30 秒与 QUIC 空闲超时 30 秒的竞态，已在 SDK Punch 传输增加 5 秒保活，不改变应用鉴权或绑定有效期。
- 新增真实 QUIC 回归：7 秒服务端空闲上限下，不发送应用心跳，等待 9 秒后连接仍可用；2 组 Punch 传输测试通过。修复后的真实做种进程空闲超过 77 秒后，新客户端下载成功，逐字节及 SHA-256 再次一致。
- 平台回归现为 22 组，全部通过；平台类型检查零错误/警告。Track/Punch 留在本机用于继续测试，临时上传客户端在验证后停止。
- 未验证公网 NAT/TURN、平台新发布到授权导出的完整 HTTP 流程；本轮没有撤回旧版本或创建新授权。
