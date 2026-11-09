# @q2q/client-web

浏览器客户端 SDK，开发中、尚未发布。当前实现已选择 `File` 的范围读取，以及文件句柄的乱序写入、提交与独占访问；已提供 OPFS 暂存、流式导出与清理；已实现版本化下载检查点及逐块恢复校验；已接入 `createClient`、HTTP 下载任务、暂停/恢复与受限来源刷新；WebRTC 内部传输层已接线并完成部分互通验证，已接入 Client 的 P2P 下载、暂停/恢复和 HTTP 回退；完整文件 Web 发布已接线并完成本地互通；下载中已提交块的显式共享已实现；主站分享接线已完成并上线；真实 Safari 双端跨设备互通仍在排查。设计与执行项见 [SDK 方案](../../todos/web-sdk.md)。

```ts
import { fileSource } from '@q2q/client-web';

// file 由应用的文件选择事件提供；SDK 不弹选择器。
const source = fileSource(file);
const reader = await source.open();
try {
  const length = Number(source.info.size < 1024n ? source.info.size : 1024n);
  const bytes = await reader.read(0n, length);
  // 把 bytes 交给后续校验/传输模块。
} finally {
  await reader.close();
}
```

每个 open 创建独立读会话。每次读取默认最多 4 MiB（可通过 maxReadBytes 调整），不预读整个文件；总体并发/内存预算由后续任务调度层约束。偏移使用 bigint，转换到浏览器 number 前检查安全范围。零长度读取只允许在文件范围内，包括 EOF。

AbortSignal 取消会抑制迟到结果，但不能强行终止 Blob 的宿主读取。close 拒绝新读取，令在途结果返回 TASK_CANCELLED，并等待宿主读取结束；关闭一个会话不影响其他会话。适配器面向 File 快照，不保证检测所有磁盘内容变化；内容一致性仍需种子/Merkle 校验。

## 文件写入

```ts
import { fileDestination } from '@q2q/client-web';

// handle 已由应用通过用户操作获得授权；工厂是异步的。
const destination = await fileDestination(handle);
const writer = await destination.open({ mode: 'create', size: 4n });
try {
  await writer.write(2n, new Uint8Array([3, 4]));
  await writer.write(0n, new Uint8Array([1, 2]));
  await writer.commit(); // 关闭流后才确认提交；仍持有目标的 SDK 独占锁
  const receipt = await writer.finish(); // 完成并释放锁
} finally {
  await writer.abort(); // 已完成时无操作；失败时清理资源
}
```

- 非空文件默认拒绝新建写入；应用明确取得覆盖同意后可设置 `overwrite: true`。`mode: 'resume'` 保留已有字节，但这里只检查目标长度；内容根、原检查点和块验证属于上层，不能按长度相同就宣称恢复成功。
- 接受的写入按顺序执行，并复制调用者缓冲。默认最多保留 8 MiB 待写数据（可配置 `maxPendingBytes`），超出返回 `RESOURCE_LIMIT`，调用者等待在途写入完成后重试；非法范围不会让写入会话失效。宿主写入/提交失败则终止会话并清理。
- `commit()` 关闭流才确认提交。后续写入惰性重开并保留已有内容；每次关闭/重开可能复制整文件，不应按每个小块提交，当前 1 GiB 导出只验证一次连续写入；不将其当作频繁重开整文件的性能承诺。`finish()` 只确认存储提交，不验证种子或所有块是否完整；可信完成由上层负责。
- `abort()` 阻止尚未执行的写入/提交，等待在途宿主操作并丢弃当前未提交内容。已经进入宿主 close 的提交无法强行撤回，可能完成；abort 等其结束后才释放锁，finish 在此竞态不会报告完成。
- 外部用户文件通过同源 IndexedDB 保存随机 ID 与句柄映射，`isSameEntry()` 识别别名；OPFS 则对根目录解析出的私有相对路径生成稳定目标 ID，不依赖句柄结构化克隆。SDK 缓存使用随机且不复用的目录名；OPFS 目标按逻辑路径标识，外部文件仍按实体句柄标识。Web Locks 在提交间隔仍持有目标锁，不同文件可并发。该锁约束同源且遵守 SDK 锁协议的调用方，不能阻止外部编辑器、不同源程序或不遵守协议的原始写入。清除站点数据会丢失用户文件映射和恢复记录，恢复仍须重新授权并校验。
- 工厂需要 Web Locks 与 OPFS 根目录解析能力；外部文件还需要 IndexedDB 支持句柄序列化，不支持时明确报错，不静默降级为不安全的无锁写入。SDK 不主动弹出授权 UI，权限失效返回结构化错误。
- 外部用户文件返回 `user-file`；若传入的是 OPFS 文件句柄，返回 `staged`、`needsExport: true`，不误报系统下载目录已保存。专用 OPFS 暂存与导出见下节；下载检查点及恢复会话见下节。

## OPFS 暂存与导出

```ts
import { opfsCache, fileDestination } from '@q2q/client-web';

const cache = await opfsCache();
const entry = await cache.create({ name: 'example.bin', size: 4n });
const writer = await entry.destination.open({ mode: 'create', size: entry.size });
try {
  await writer.write(0n, new Uint8Array([1, 2, 3, 4]));
  await writer.commit(); // Dedicated Worker 的同步句柄执行 flush
  const receipt = await writer.finish(); // staged，仍待导出
} finally {
  await writer.abort();
}

// 应用保存 entry.id。刷新后可以重新打开同一个缓存，不必持有旧 JS 对象。
const reopened = await cache.open(entry.id);
const target = await fileDestination(alreadyAuthorizedSaveHandle);
await reopened.exportTo(target, {
  signal: controller.signal,
  onProgress: ({ copiedBytes, totalBytes }) => render(copiedBytes, totalBytes)
});
// 导出成功也保留原缓存，由应用明确选择何时清理。
await cache.remove(entry.id);
```

示例中的文件授权、AbortController 和 UI 由应用提供，不是 SDK 的隐式全局变量。

- 每个活跃写入会话由一个 Dedicated Worker 持有一个同步访问句柄；Worker 按 SDK 私有路径自行打开文件，不跨 Worker 传递文件句柄，兼容不支持句柄克隆的引擎。随机写不反复复制整个缓存。`commit()` 显式 flush，`finish()` 在 flush、关闭句柄并提交版本化完成标记后返回。零长度文件支持同一流程。
- 同步 OPFS 写入直接修改内容，abort 无法回滚；取消或崩溃后可能残留未提交字节，条目仍是 partial，不能通过普通 source 读出或导出。检查点恢复会重新校验登记块，不能信任文件长度。若取消发生在已经启动的最终元数据提交期间，完整的完成标记也可能落地，但取消调用不报告成功完成。
- partial / complete 是存储状态，不是 Merkle 验证结论；finish 检查实际长度，但不会检查块覆盖、稀疏洞或内容摘要。上层只有验证全部内容后才能调用 finish。普通 source 只读；显式 resume 写入（包括检查点恢复）会先撤回完成标记、改为 partial，重验并完成后才能再导出。重新下载其他内容应创建新条目。
- `entry.source` 提供完成缓存的有界范围读取，读会话持有目标锁，必须 close。exportTo 默认按 1 MiB 分块复制，支持 1 字节至 4 MiB 的 chunkBytes、取消与只读进度；不会整文件转成 Uint8Array。目标自己的待写预算也可能限制块大小。
- 导出失败或取消保留源缓存，错误不会变成虚假的“已保存”；可以重新授权目标后重试。若目标本身也是 OPFS，返回 staged 而不是 user-file。当前不提供自动 Blob 导出或浏览器下载管理器的落盘确认。
- remove 只删除指定 SDK 缓存目录及其目标映射，不删除用户文件、任务历史或下载检查点。活动写入、读会话和导出期间返回 DESTINATION_BUSY；不存在的条目返回 CACHE_NOT_FOUND。缓存 ID 是随机不透明标识，不接受目录路径。
- 字节 flush 与元数据完成标记是分步操作，不能当作跨文件事务。完成标记缺失时不猜测成功；读取时检查元数据版本与内容长度。站点数据清理、浏览器回收和真实磁盘故障仍可能丢失缓存。
- 默认 Worker 用 `new URL('./opfs-worker.ts', import.meta.url)` 交给打包器处理；也可通过 `opfsCache({workerUrl})` 指向实际构建产物。模块 Worker 需要匹配同源/CSP/MIME 配置。启动握手失败或脚本不可用会报错并清理资源；Web SDK 导入时不启动 Worker。真实第三方打包器兼容性仍待 C5 验收。

## 检查点与恢复校验

```ts
import { browserState, createBlockVerifier, resumeCheckpoint } from '@q2q/client-web';

// 种子与外置证明由应用提供；内联证明的种子可省略 proof。
const verifier = await createBlockVerifier({ seed: seedBytes, proof: proofBytes });
const state = await browserState({ namespace: 'my-app' });
const checkpoint = await state.create({
  content: verifier.content,
  targetId: entry.destination.targetId
});
// 刷新后重新取得 entry、原种子和证明，并复用保存的 checkpoint.id。
const session = await resumeCheckpoint({
  state,
  checkpointId: checkpoint.id,
  verifier,
  destination: entry.destination
});
try {
  // 应用取得完整块后调用；这里会再次对照原种子校验，而非信任调用方。
  await session.writeBlock(blockIndex, blockBytes);
  await session.commit(); // 先提交文件字节，再事务更新位图
  // 其余块齐全后才能 await session.finish()；尚无网络下载器。
} finally {
  await session.abort();
  await state.close();
  verifier.close();
}
```

- StateStore 保存内容 info_hash、长度、块大小、目标引用、版本、修订号和提交位图；不默认保存原始种子、证明 URL、账号密钥或文件内容。恢复时应用必须再次提供原种子和证明。缓存 ID、checkpoint.id 与 targetId 分别标识缓存、恢复记录和存储目标。
- `createBlockVerifier` 复用 `@q2q/wasm` 的 Rust Seed/Merkle 代码。外置叶子表必须匹配种子根，块长度与块哈希必须正确；浏览器首次 Wasm 初始化使用单一 Promise，避免并发实例覆盖绑定。可指定 wasmUrl，首次成功初始化后同一 realm 共用该模块。
- `resumeCheckpoint` 同时持有检查点租约与目标写入租约。内容身份、长度、块大小或目标不匹配时，在打开目标写入前失败。只检查位图登记的块；损坏/缺失块清零，未登记字节即使存在也不会直接增加可信进度。
- 重验成功的块会写回当前写入会话并提交，确保用户文件流的临时副本确实包含已验证字节，再 CAS 更新修复后的位图。每次恢复推进修订号；取消后的旧回调不能在 abort 返回后继续提交。
- `writeBlock` 只接受完整的合法块，内容错误不毒化整个会话。缓存写入完成但 commit 尚未成功时，检查点进度不增加。commit 始终先刷新字节，再执行 IndexedDB 事务；二者不是原子事务。第二步失败后下次按旧位图重验，不把裸文件长度当作完成证明。
- finish 要求全部块已校验，并完成目标提交；检查点仍保留，便于处理中断的完成交付。应用可显式 discard(checkpointId) 只清恢复记录。已完成缓存被重新恢复时会撤回源可读状态，直到重验和再次 finish。
- 原始 `state.read/list` 中的 committedBytes 是上次提交的提示，不是本次重验结果；活会话以 session.snapshot() 为准。返回位图是副本，调用者修改它不会变更持久记录。
- 低层 state.lease(id) 提供 snapshot、带 expectedRevision 的 replace 和 close；同源跨标签页独占，活动记录不能 discard。CAS 在同一个 IDB 读写事务中比较和更新，未知格式或损坏位图明确报错，不静默重置。显式 discard 可清除损坏记录，但不会删除目标文件。
- list 使用 after/limit 分页（默认 32，最多 128）；namespace 隔离同源应用的数据，不是跨用户鉴权边界。close 等待已有数据库操作收敛并释放检查点租约；租约 signal 同时通知恢复会话停止 IO，防止空闲 Worker 长期持有文件。应用仍应先 await session.abort()，再关闭注入的 state；state.close 不替代等待完整文件任务关闭。浏览器直接清站点数据可能等待活动同步句柄，应用自己的清理入口必须先停止任务。
- 当前最多 524,288 块，位图最多 64 KiB，外置证明最多 16 MiB；这只是有界元数据预算，不是已验证的浏览器文件大小承诺。恢复读取上限为每块 4 MiB，会话默认保留最多 8 MiB 待写副本。完整块校验目前同步调用 Wasm，UI Worker 编排与大文件性能仍待后续阶段验证。

check 与 test:browser 会先生成 Wasm 产物，需要已配置 wasm32-unknown-unknown target 和匹配的 wasm-bindgen CLI；沿用仓库的 Wasm 构建脚本。

## 测试

```sh
npm run check --workspace @q2q/client-web
npm test --workspace @q2q/client-web
npm run test:browser --workspace @q2q/client-web
npm run test:http --workspace @q2q/client-web
npm run test:large --workspace @q2q/client-web
VITE_RESOLVE_FROM=apps/platform/package.json npm run test:bundle --workspace @q2q/client-web
```

浏览器测试使用标准 `@playwright/test` runner，默认 Chromium 与 WebKit 项目；每个契约用例有独立磁盘 profile。失败保存截图/trace，HTML 报告放在仓库 `.tmp/client-web-playwright/report`，大文件结果作为 JSON 附件保留。大文件测试不启用追踪，避免影响资源测量。

可设置 CHROME_PATH 指定浏览器；未设置时使用匹配 Playwright 的安装版本。新增引擎时建议 `PLAYWRIGHT_SKIP_BROWSER_GC=1 npx playwright install chromium webkit`，避免安装器清理其他任务已有的缓存。通过 `TEST_FIREFOX=1` 和 `--project firefox` 可以显式尝试 Firefox，不把本机启动失败标成 SDK 不支持。

`test:large` 在临时磁盘 profile 中流式生成 100 MiB/1 GiB，提交四分之一后真正重启浏览器，恢复校验后继续写完、逐块读回并导出；要求至少 4 GiB 临时可用空间。比较缓存与导出文件的 Q2Q 种子身份，完成后删除测试数据。Chromium CDP 覆盖临时 origin 的配额，验证 STORAGE_FULL，不填满用户磁盘。

`test:bundle` 用仓库已有 Vite 构建真实包导出，再以仅允许同源脚本/Worker 和 wasm-unsafe-eval 的 CSP 验证默认资源加载。VITE_RESOLVE_FROM 相对仓库根目录解析，可选择工作区所用的 Vite；SDK 运行时代码不依赖该应用。

系统保存是单独的交互测试：`npm run test:save --workspace @q2q/client-web` 会打印临时目录并打开可见测试页，需要在原生保存窗口选择该目录。也可设置 MANUAL_BROWSER=1 后在已连接的测试浏览器打开打印的网址。测试通过系统路径独立读回文件，与浏览器侧字节及 SHA-256 比对；清理只删除本次创建的测试目录。

2026-10-28 桌面验收：标准 runner 104 项通过、2 项 CDP 专用场景在 WebKit 跳过；另有 10 项 Node File/Blob 测试通过。Chromium 148 的正常磁盘 profile 已验证到 1 GiB，WebKit 26 的共用存储契约及 Vite 8.3.4 产物通过；WebKit 没有系统保存选择器，不能据此宣称完整主站下载体验已支持 Safari。Firefox 在本机启动失败，Safari.app、移动端、真实撤销文件权限及物理磁盘耗尽尚未验收。该轮只验收存储；HTTP 下载验收见下文。详细矩阵与资源数值见设计文档 C5 记录。

包暂为 private；源码出口沿用仓库现有方式，npm 发布产物尚未发布。


## HTTP 下载任务

```ts
import { createClient, browserState, opfsCache } from '@q2q/client-web';

const state = await browserState({ namespace: 'my-app' });
const client = createClient({ state });
const seed = await client.parseSeed(seedBytes); // 只解析，不访问种子来源。
const cache = await opfsCache();
const entry = await cache.create({ name: seed.info.name ?? 'download', size: seed.info.size });
const task = client.download({ seed, destination: entry.destination });
const unsubscribe = task.subscribe(snapshot => render(snapshot));
try {
  const result = await task.wait();
  // result.output.kind === 'staged' 表示完成暂存；应用可提示用户选择真实保存位置，
  // 然后调用 entry.exportTo(await fileDestination(handle))。
} finally {
  unsubscribe();
  await client.close(); // 等网络、写盘与检查点收敛；不会关闭注入的 state。
  await state.close();
}
```

`task.pause()` 中止在途请求，等待已经接受的写入、提交已校验块，再释放文件和检查点租约。它不是每个网络字节都可恢复的承诺；未形成合法完整块的数据会丢弃。`task.resume()` 重验检查点后继续；重复 pause/resume 对已暂停/运行任务无副作用。`cancel()` 是终止命令，不能继续同一句柄，`wait()` 返回 TASK_CANCELLED；已提交内容及记录保留，可另建恢复任务。多个 wait 观察同一结果，不调用 wait 也不会产生 SDK 内部未处理的完成拒绝。订阅者抛错不影响传输。

跨页面恢复需要显式注入 `browserState`，保存 entry.id/checkpointId 并重新提供原种子：

```ts
const entry = await cache.open(cacheId);
const task = client.resumeDownload({ seed, destination: entry.destination, checkpointId });
await task.wait();
```

默认 state 仅在 Client 内存中，close 后清空；不承诺刷新恢复。`listCheckpoints({after,limit})` 分页读取记录，`discardCheckpoint(id)` 只删除元数据。快照不含授权 URL、种子正文或本地文件路径，进度区分 receivedBytes（含重试）、verifiedBytes、committedBytes；恢复前旧记录只作提示，恢复后才报告重验进度。bytesPerSecond 是本任务自创建起的平均内容接收速率，包含暂停时间、排除证明流量。

受限来源由应用注入 `sourceProvider.resolve({content,reason,signal})`，返回 `{contentUrls,proofUrls}`。存在 Provider 时，其地址覆盖种子来源；一次运行的并发 401/403 合并为一次 unauthorized 刷新，继续失败即 AUTH_REQUIRED，不无限刷新。SDK 不发送 Cookie、Referer 或应用账号头；应用可返回短期签名 URL，不应在日志/快照暴露它。忽略取消的 Provider 迟到结果也不能重启任务。

每个 Client 的默认限制：全任务共享 2 个 HTTP 请求、最多 4 个非终态任务（包含暂停任务），16 MiB 校验数据一批提交，请求总期限 30 秒（含排队和响应体），每来源最多尝试 2 次，退避 100ms。可经 limits 调整，HTTP 请求最多 16、任务最多 64、每来源尝试最多 8；来源列表最多 8 个地址。每个下载 worker 最多保留一个完整块，写盘串行排队；不把全文件读入内存。解析中任务数也受 maxTasks 限制。外置证明先按最多 16 MiB 有界读取并验证根，再开始内容写入。

范围来源必须支持精确 206 与 Content-Range，并返回严格的完整块长度；跨源服务器须允许浏览器请求，且显式 expose Content-Range。200 仅在整个文件就是单块时接受；多块来源忽略 Range 会报 HTTP_RANGE_UNSUPPORTED，目前不做顺序整文件降级。压缩/截断/越界响应、错误偏移、416 和内容哈希不匹配不会标记完成；可用其他来源修复错误内容/证明。浏览器自动跟随重定向，SDK 检查最终响应及内容，无法检查每一个中间跳转或设置跳转次数上限。

省略 trackUrl 时 capabilities 报告 http=true、p2p=false、publish=false；显式入口及浏览器 WebRTC 可用时可尝试 P2P 下载，完整文件发布已开放；每个 Client 首版最多一个发布句柄。HTTP 集成测试使用本地 Range/CORS 服务和真实 OPFS，并非线上平台或大规模 HTTP 性能验收。Firefox、真实 Safari 与移动端仍按前述未验收范围处理。

2026-10-28 C6 验证：64 项 HTTP Playwright 用例在 Chromium 148 / WebKit 26 全部通过，包含实际刷新恢复；既有存储与 Vite 8.3.4 打包回归 103 项通过、1 项 WebKit CDP 用例跳过，Node 10 项与类型检查通过。不是 Firefox、真实 Safari 或线上 HTTP 负载验收。


## Track / Punch 控制连接

```ts
import { createClient } from '@q2q/client-web';
const client = createClient({ trackUrl: 'https://track.example.com' });
try {
  const network = await client.connectNetwork();
  const unsubscribe = network.subscribe(snapshot => console.log(snapshot.state));
  // 自动维护心跳；offline 后可显式再次 connectNetwork()。
  unsubscribe();
  await network.close();
} finally {
  await client.close();
}
```

此入口本身维护控制连接；下载任务会复用它发现 Peer，再建立独立 Punch/WebRTC 连接。publish 仍为 false。每个 Client 使用独立临时 Peer 身份，显式重连复用它；关闭 Client 释放身份。没有平台登录依赖，密钥与 TURN 凭据不进入快照。并行 connectNetwork 共用初始化，心跳不重叠；断线变 offline，调用者决定是否重连。关闭会排空心跳、尽力 exit 并清理 WebSocket/计时器；Track 记录按服务器 TTL 到期。

只消费 Track 下发的 endpoints 列表，通过共享校验筛选 WSS（HTTP loopback 页面测试可用 WS），保留完整端口、路径与 query，不从 QUIC 地址推导。浏览器公信证书认证 WSS 代理入口；原生 WSS 同样使用公共 CA 校验，QUIC 仍校验节点公钥。Track URL 要求 HTTPS，并支持 JSON CORS 和 punch_schemes 能力集合；旧单地址协议不兼容。

Punch 控制帧最多 64 KiB；Track JSON 响应最多 256 KiB，最多 32 个待响应 RPC、8 个在途信令处理、256 KiB 发送缓冲；不无限排队。尚未实现 RTC 时拒绝入站连接。签名和 MAC 复用 Rust/Wasm，消息序列保留 u64 精度。

运行 `npm run test:network --workspace @q2q/client-web`，会构建临时原生 Punch 并使用真实本地 workerd Track/Registry，在 Chromium/WebKit 验证控制协议；故障测试用 Playwright WebSocket 路由注入。公网证书、RTC、TURN 与跨 NAT 另行验收。


## WebRTC 传输层进度

内部 rtc-peer.ts 已实现与原生一致的双通道、Wasm 双向身份确认及 DTLS SHA-256 指纹绑定。可靠有序控制帧保持原始 JSON；块帧复用 Rust Piece 编码，数据通道为 negotiated stream 1 / unordered / maxRetransmits=0，重传由后续 Q2Q 块调度处理。认证前不向应用交付内容；发送背压、收件箱、早到候选和待处理工作有上限，关闭会取消并清理资源。

`npm run test:rtc --workspace @q2q/client-web` 构建 Wasm，运行 Chromium/WebKit 浏览器契约，并通过临时 TCP 信令桥与真实原生 SDK WebRTC/身份交换互通。测试桥只用于测试，不进入 SDK 产物，不经过 Track/Punch；数据在真实 DataChannel 上双向交换。

当前 Chromium ↔ 原生双角色通过；macOS Playwright WebKit ↔ 原生停留在 ICE 建连阶段，原因未定，默认跳过该两例，可设 `TEST_NATIVE_WEBKIT=1` 复测。WebKit 浏览器双端契约仍默认运行。共享帧、u64 边界及 transcript 使用 `npm run wasm:check -- 1` 验证。

此传输层已接入 Client 下载调度，完整文件、丢包修复、暂停恢复与取消已在本地 Chromium ↔ 原生 SDK 验证。Web 发布已通过本地 Chromium/原生互通，TURN、跨 NAT 及发布侧公网 WSS 完整链路仍待验收。


## P2P 下载

```ts
const client = createClient({ trackUrl: 'https://track.example.com', state });
const seed = await client.parseSeed(seedBytes);
const task = client.download({ seed, destination });
try {
  await task.wait(); // 验证、提交目标/检查点并关闭本任务的 Peer 后才完成
} finally {
  await client.close();
}
```

入口显式配置；不会隐式访问种子里的 Track 地址。未配置入口沿用 HTTP 模式。capabilities.p2p 表示有显式入口及 WebRTC 宿主能力，不代表网络、任意种子或所有浏览器互通已经验收。当前只验证 Chromium 与原生 SDK，WebKit/native ICE 仍是已知待验证项。

任务优先尝试有界 Peer 列表，失败再使用现有 HTTP 来源；不是原生 SDK 完整的延迟竞争/来源调度策略。Track 查询与心跳共用顺序 MAC 序列；每次 dial 使用独立 Punch WebSocket，验证 accept 票据及 DataChannel 身份，TURN 凭据不进入快照。每次查询最多读取四页候选、依次尝试八个 Peer；文件索引刚发布尚未收敛时短暂重查三次。全 Client 默认最多 2 个 Peer 连接，可用 limits.peerConnections 调整至 1–16，与 HTTP 请求额度分开。

每个 Peer 一次请求一个块。块拼装、重排/重复冲突检查、缺失区间、线协议与位图解析复用 Rust；丢包通过原有 PieceNack 修复。重复数据和无关 Have 不延长无进展期限，UploadWait 沿用原生上传节奏。暂停/取消中止在途传输；已验证块写入、目标及检查点提交后才发 Have/BlockDone。每块提交可能比 HTTP 批次提交开销更高，尚未做 P2P 大文件性能测量。

优先复用已验证的内嵌叶子表或 HTTP 外置叶子表；没有可用叶子表且配置了 Peer 网络时，使用 Peer 的分块证明直接校验并保存紧凑见证，不再依赖 HTTP 证明地址。P2P 最多 65,536 个块。内存检查点仅当前 Client 可用；跨 Client/刷新恢复仍需 browserState、同一种子和原目标授权。

`npm run test:p2p --workspace @q2q/client-web` 使用临时真实 Workers Track/Registry、原生 Punch 和原生 SDK publication，没有在线服务操作。浏览器通过公开 Client API 下载；测试经独立 SHA-256 和逐字节比较检查 OPFS 导出。丢包/损坏在收包接口注入，修复数据仍从真实原生端重传。WebKit/native 用例默认跳过，设置 TEST_NATIVE_WEBKIT=1 可诊断复测。


## 纯 Peer 证明的恢复状态

外置种子现在可仅凭种子根、Peer 数据及原有 Proof 帧下载。Wasm 复用 primitives 的 Merkle 校验，将有效证明规范化为兄弟节点见证；不会把“Peer 已证明”或本地缓存叶子哈希当作可信标记。证明节点不得覆盖当前数据及其计算路径上的祖先；有效的展开兄弟节点证明也能规范化保存。

browserState 自动将原有 `q2q-checkpoints-v1` 数据库升级到 IndexedDB 版本 2，保留版本 1 检查点记录，新增 witnesses 表。旧版 SDK 显式打开数据库版本 1 时会得到 CHECKPOINT_VERSION，需要更新 SDK；不通过清空数据降级。证明行按 namespace/checkpoint/index 定位，格式版本 1，只保存紧凑兄弟哈希。当前最多 65,536 块，每条最多 534 字节，读取/写入防护上限 640 字节；大量证明仍会增加元数据配额和恢复时间，未据此宣称大文件性能验收通过。

下载按“验证字节与证明 → 写入并提交目标 → 同一 IndexedDB 事务保存见证与检查点位图 → Have/BlockDone”执行。文件与数据库之间不存在跨存储原子事务；元数据配额失败时，目标可能已写入，但位图与见证一起回滚，任务不会报告这些块已提交。

恢复按块读取本地见证和实际文件字节，用重新选择的种子根验证。证明缺失、损坏、跨块或文件字节被改动时撤回对应位，之后重新下载；未知未来证明版本返回 CHECKPOINT_VERSION。内置存储在重验旧块时不向 Track/Punch/CDN 补取证明，证明不会携带种子来源、URL 或会话凭据。删除检查点同步删除见证，文件仍由目标存储单独管理。

CheckpointLease 新增可选 `witness(index)`，`replace(revision, bitmap, witnesses?)` 的第三参数为只读 Map<number, Uint8Array>；适配器必须在位图 CAS 的同一事务写入证明、删除撤回位的证明。更新数据会复制，每次最多 1,024 条。旧适配器不实现 witness 方法仍支持原有完整叶子表模式，纯分块证明模式明确报 UNSUPPORTED_CAPABILITY。默认内存状态提供相同契约，但不跨刷新；browserState 可跨刷新恢复。

ContentVerifier 可声明 requiresWitness，并用 verify(index, bytes, witness) 重验；CheckpointSession.writeBlock 接受可选第三个见证参数，提交前会再次验证。HTTP 内容回退仍要求可用的完整叶子表；纯分块证明模式不把没有相应证明的 HTTP 字节登记为可信内容。


## 公网浏览器下载显式验收（opt-in）

默认测试不会创建线上会话。`tests/public-p2p.spec.mjs` 需要明确授权生产目标后，设置
`TEST_PUBLIC_P2P=1` 与 `Q2Q_PUBLIC_TRACK_URL=https://...`，再用 Playwright 的
`--grep @public --project chromium --workers 1` 执行。它通过原生 SDK 发布有界生成内容，
浏览器使用真实 Track/WSS/WebRTC，分别覆盖完整下载、暂停恢复及纯 Peer 证明的 reload。
测试不放宽 TLS、路由拦截或线上限流；每个场景使用独立发布会话并等待 65 秒恢复配额。
原生辅助程序远端访问还需独立的 Q2Q_BROWSER_ALLOW_REMOTE=1，夹具仅在显式 opt-in 时设置。

2026-11-08：香港公信 WSS 的完整下载与暂停恢复阶段通过；拆分发布会话后，纯 Peer
证明跨刷新恢复连续两次通过（含最终独立 SHA-256）。文件 786,449 字节，内容/证明均无
HTTP 来源，文件字节走 WebRTC。最初串联场景遇到 join/exit rate_limited，配额等待后
长会话末段仍遇到 CONTROL_TIMEOUT，原因尚未确定；隔离成功不等于长会话问题已根治。
两个 Peer 均在本机，不计跨 NAT/强制 TURN、手机、WebKit/native 或主站 P2P UI 验收。


## 本机完整文件发布

```ts
import { createClient, fileSource } from '@q2q/client-web';

const client = createClient({ trackUrl: 'https://track.example.com' });
const publication = client.publish({ source: fileSource(selectedFile) });
const unsubscribe = publication.subscribe(snapshot => render(snapshot));
try {
  const { seed } = await publication.ready();
  // 应用负责把 seed.toBytes() 保存为 .q2q 并交给下载方。
  // 这里不是上传到平台，也不表示接收方已经收到完整文件。
  await publication.pause();  // 停止入站服务、撤回来源，保留种子
  await publication.resume(); // 同一内容身份重新宣告
  // 应用等待用户选择停止分享，而非导出种子后马上关闭。
} finally {
  await publication.close();
  unsubscribe();
  await client.close();
}
```

- publish 立即返回 Publication，不等待哈希/发现完成；ready 是首次准备和宣告成功的
  同一 Promise。只有快照 state 表达当前是否仍在发布。准备期间也可以 pause/close；
  暂停中的准备首次恢复会重读文件，不继承未经验证的进度。
- 首版每个 Client 一个完整文件发布，准备/暂停任务均占用总任务额度。默认上传连接 2、
  文件读取并发 1，分别用 limits.uploadConnections / uploadReads 配置（各最多 16），
  与 peerConnections 下载额度独立。每条连接一个块窗口，内存不随文件长度增长为全文件。
- 固定 256 KiB 分块，最多 65,536 块；使用 Rust/Wasm 增量 SeedBuilder、Merkle tree
  和原生证明/线协议。种子首版内联叶表，没有把全部内容读入内存，也没有依赖 HTTP 证明源。
  这些是保护预算，不是已验收的浏览器大文件承诺。
- Track 宣告/续约/撤回与查询/心跳共用串行控制预算，换回来的 session_key/seq 在
  同一串行区更新。在线网络会话定时续约持有；网络离线将发布置 failed，不无限后台重连。
  撤回网络失败会拒绝命令，服务端记录也受 TTL 约束，不承诺远端原子删除。
- 入站 connect 先由 Rust 验票据签名、目标、audience、connection 与内容哈希，再建立
  Peer；DataChannel 必须完成 Peer 签名/DTLS 指纹认证才处理业务帧。复用原有 Bitfield、
  Request/Proof、1024 字节 Piece、PieceNack/RequestProof 与完成握手，不引入另一个协议。
- 每个上传连接独立打开 source reader。发送块前重新对照种子验证字节；内容变化或源读取
  失败停止发布并收敛资源，不能用旧种子宣告并继续发送新内容。重传发送的是该连接已验证
  的有界块副本，BlockDone 后释放。源文件句柄仅为读取能力，不作为可信内容证明。
- sentBytes 是包含重传的发送量，不是接收方落盘量。快照不含私钥、票据、源地址或凭据；
  观察者异常隔离，高频准备/发送通知约 100ms 合并，状态转换即时通知。
- 关闭 publication 不关闭借入的 ContentSource 全局对象，只关闭自己打开的读会话；
  Client.close 先收敛自己的发布再关闭网络。原生或浏览器的外部编辑器不受 SDK 锁约束。

本地验证：Chromium 发布到 Chromium、WebKit 发布到 WebKit 的完整字节/SHA-256，
Chromium 发布到真实原生 SDK 的完整文件通过。准备取消/暂停恢复、撤回可见性、空文件、
observer 重入关闭、源变化拒绝、丢片 NACK 修复通过。WebKit/native ICE 仍显式跳过。
协议测试用有界内存目标隔离 WebKit 隐私 profile 的 OPFS 限制；原生下载测试实际写盘，
生产 OPFS/检查点由独立持久 profile 套件验证，不能将内存目标当作系统保存验收。

`npm run test:publish --workspace @q2q/client-web` 使用临时本地真实服务；CI 已接线，
不访问生产。主站仍未开放分享/P2P 页面；下载中的部分块共享见下节，多个发布任务、上传限速、
发布侧公网/跨 NAT/TURN、大文件发布性能仍未完成或未验收。


## 下载中的已提交块共享

普通 download / resumeDownload **默认不共享**。应用明确选择后使用：

```ts
const { transfer, publication } = client.downloadShared({ seed, destination });
// transfer 是下载句柄；publication 是独立可关闭的来源宣告/上传句柄。
const unsubscribe = publication.subscribe(snapshot => renderSharing(snapshot));
try {
  // ready 仅说明存储恢复检查完成、部分来源宣告成功，不说明已有块可发送。
  await publication.ready();
  await transfer.wait();
} finally {
  await publication.close();
  unsubscribe();
}
```

- 首版共享绑定**这次活跃的下载写入会话**。下载暂停、取消、失败或最终完成前，先停止
  上传、撤回来源并关闭共享句柄，再释放/完成目标；不在下载完成后隐式长期做种。
  关闭 publication 不取消 transfer，下载可以独立继续。两句柄分别占任务额度，
  上传继续使用独立 uploadConnections/uploadReads 预算，与下载网络额度隔离。
- 同一句柄下载暂停后 resume 可以继续下载，但已关闭的共享不会自动复活。如需再次
  显式共享，可结束旧 transfer 后，用相同 StateStore / seed / destination / checkpointId
  调用 resumeDownloadShared，取得新的一对句柄；先重读校验已登记块再宣告可用性。
  完成后的持续分享使用显式完整源 publish，不复用已经关闭的写入句柄。
- Track 仅宣告 complete=false；初始 Bitfield 来自本次恢复重验后的提交位图。
  “字节写入/flush → 位图与见证 CAS 成功”之后才将新增位变为可用并发送 Have。
  已接收、已校验但未提交、metadata 提交失败、恢复时撤回的块不能作为上传来源。
- 上传不再独立打开第二个 partial OPFS 文件句柄，不绕过写入目标锁。CheckpointSession
  的 readCommittedBlock 把有界读回串行排在目标 IO/CAS 后，只读取当前已提交位，
  再验证实际字节与对应证明，返回独立块副本后才在网络上发送，不持存储队列等网络。
  自定义目标的 read/write/commit 契约仍须真实有效；SDK 不承诺修复不遵守契约的适配器。
- 完整叶表模式复用共享 Rust ProofTree；只有 Peer 证明的外置种子则从已原子持久化
  的紧凑见证重建**原生 Control::Proof**，先验证实际字节，不能只信位图/见证存在。
  证明编码与 Merkle 验证都在 Rust/Wasm，没有另写 JS 哈希算法。缺块请求按现有 Reject
  协议拒绝；磁盘内容或见证损坏会停止共享，后续下载也会遇到存储失败而不是传播坏内容。
- 动态 Have 和上传待发队列有界；网络变慢时关闭对应连接，不为每块积累无限通知。
  下载器仍保留现有有界候选/HTTP 回退策略，本节没有新增对“唯一部分来源尚未拥有的
  未来块”无限等待或重新发现策略，不能据块共享声称原生完整多 Peer 调度已移植。
- 当前每 Client 仍最多一个发布/共享内容，最多 65,536 块；跨刷新共享不会自动后台恢复，
  必须重选种子/授权并显式 resumeDownloadShared。暂停下载保留检查点，不静默删除文件。

本地 partial 专项验证内存目标、Chromium 真 OPFS、metadata CAS 失败不发布块、动态
Bitfield/Have、缺块拒绝、关闭分享不取消下载、暂停/完成先撤回、检查点新会话重验、
已提交内容被改坏停止宣告，以及原生 origin → 浏览器中间节点 → 浏览器接收已提交块的
Peer-only proof 转发。后者传的是块与证明，不把低层分块验收冒充三端完整文件级调度。
`npm run test:partial --workspace @q2q/client-web` 只用临时本地服务，CI 已接线；
WebKit/native 与私有上下文 OPFS 两项明确跳过，不宣称已兼容。


## 连接失败诊断

`Q2QError.diagnostic` 与任务 `snapshot().error.diagnostic` 是可选的只读连接上下文。
原有 `code`、`retryable` 不变；没有上下文时该字段缺省。诊断在清理 RTC 前截取，包含
失败阶段、RTC/ICE/信令/两个 DataChannel 状态和本地/远端候选计数。阶段区分 Punch
连接/接受/offer/answer、RTC 描述/候选处理、建连、身份握手和传输；它是失败时的本地
状态，不等于已证明的根因。候选计数不表示候选已可用，也不证明使用了 TURN。

字段经过白名单与长度固定的枚举/有界整数检查，不包含 SDP、候选地址、来源 URL、
Peer 身份、原生异常正文、票据或 TURN 凭据。SDK 不自动打印或上传诊断。应用可在
失败任务详情展示；该信息不写入检查点，刷新后不承诺保留。仍需真实设备重现才能
判断 Safari 的具体互通问题，不能以 Playwright WebKit 测试通过替代 Safari 验收。
