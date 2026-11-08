# JavaScript 客户端 SDK 设计与执行项

状态：2026-10-29，Web 客户端采用 SDK 先行：先明确放在 `packages` 的独立浏览器 SDK，再实现主站。本文包含设计草案、实施进度与验收计划；`packages/client-web` 已建立，File 读取、文件句柄写入、OPFS 缓存及检查点模块已实现并通过本地契约与 Chromium/WebKit 存储测试，HTTP/P2P Client 下载、下载检查点与逐块恢复已实现并完成本地 Chromium ↔ 原生全链路验证；纯 Peer 外置证明及跨刷新恢复已实现；Web 发布、公网路径和 WebKit/native 仍未完成，包尚未发布。主站交互见 [web-client.md](web-client.md)。

## 1. 定位与包边界

采用两个独立 npm 包：`@q2q/client-web` 对应 `packages/client-web`，`@q2q/client-node` 对应 `packages/client-node`。Web 先实现并独立交付，Node 使用 napi-rs 绑定现有 Rust `q2q-core`，不建立空入口或在实现前发布包。两端均不依赖 Svelte、SvelteKit、Q2Q 账号或特定页面结构。

两包独立构建、分发，通过共同契约和行为测试约束 Client/Transfer/Publication 一致性。Web 不加载 Node 原生依赖，Node 不依赖 DOM；文件选择、权限和恢复能力按环境表达，不强求完全相同。

共享代码按实际职责提取，不建立 `client-common` 杂物包。可提取 `@q2q/client` 承载环境无关的公共契约；已有 `@q2q/wasm` 继续承载 Rust 内容与协议绑定。`@q2q/storage`、`@q2q/transport` 仅在出现实际跨包消费者与共用代码时建立，不为 Node 原生内部能力重复搭建 TS 存储或 WebRTC 引擎。环境包重新导出使用者需要的类型，普通用户只安装对应环境包。

```text
apps/www                    选择文件、弹窗、列表、文案、主题、平台账号
       ↓ SDK 命令 / 任务句柄 / 只读快照
packages/client-web     浏览器客户端、任务与资源生命周期、存储/网络适配
       ↓ 共享内核与薄绑定
@q2q/wasm + Rust 共用逻辑    内容身份、种子、Merkle、协议与可复用的传输规则
```

| 层 | 承担的工作 | 不承担的工作 |
| --- | --- | --- |
| 主站应用 | 文件选择和保存授权、任务展示、用户偏好、登录及项目选择 | Peer 协议、块调度、校验和重试状态机 |
| Web SDK | 分享/下载任务、发现和信令、传输协调、进度、取消、恢复及资源预算 | 弹窗、按钮文案、套餐购买、平台 Cookie 与长期 S3 密钥 |
| 浏览器 host 适配 | File/存储、Fetch、WebSocket、WebRTC、Worker 消息 | 重新定义种子格式、签名消息或内容校验算法 |
| Rust 共享逻辑与 Wasm | 内容和协议正确性、可提取的传输状态与调度规则 | 直接依赖 DOM、原生文件路径或浏览器 UI |

先设计清楚真实需要的边界，不先建立一组空包或新 crate。`packages/ui` 只放展示组件。平台登录和上传适配由应用提供，不把平台业务耦合进通用 SDK。

## 2. 当前可复用内容

- `q2q-primitives`：内容身份、种子、Merkle、Peer wire 等共享领域能力。
- `q2q-core`：已有原生 Client、Transfer、Publication；下载存储、调度、进度、Peer 与信令分别已有模块，但仍依赖原生运行时。需要逐项核对实际可抽取部分。
- `q2q-wasm`：目前只有种子解析、构建和部分证明操作，不是完整 Web SDK；块校验、协议/认证绑定和浏览器传输仍需设计。
- Track HTTP、Punch WebSocket、WebRTC 与原生互通基础：可沿用现有协议，但必须真实验证浏览器 API、鉴权、预检、证书和传输行为。

原则：共享 Rust 的内容与协议实现，优先提取运行时无关的传输规则；JS/TS 承接浏览器 API。不能把现有原生 Rust Client 直接编成 Wasm 当作已解决，也不能在页面里重写第二套 SDK。具体提取范围在能力验证中确定并记录。

## 3. 公共接口草案

以下为完整客户端公共 API 首稿，用于审查调用体验及生命周期；目前存储基础类型、StorageError、fileSource、fileDestination、opfsCache、browserState 与低层检查点恢复会话已有实现，其中 HTTP createClient/Transfer 已在 C6 实现，Publication/P2P 仍未实现，不代表 SDK 已支持下述全部行为。低层适配器类型、Worker 编码和资源默认值仍待细化。

### 3.1. 公共入口

| 入口 | 内容 |
| --- | --- |
| `@q2q/client`（按需提取） | 运行时中立的公共契约和结构化错误；由环境包重新导出 |
| `@q2q/client-web` | `createClient` 和 File、文件句柄、浏览器状态存储等适配器 |
| `@q2q/client-node` | 后续实现的 `createClient` 和文件系统适配；实际可用后才导出 |

ESM 优先。导入模块不启动 Worker、连接网络或读取文件；`createClient()` 才初始化当前宿主的能力。Web 入口在 SSR 中可导入，调用浏览器初始化时若环境不支持，返回结构化错误。

同名 Client、Transfer 和 Publication 使用相同生命周期契约；环境差异由各环境包的文件/存储适配表达，不用 `File | string | Buffer | ...` 混合参数让调用者猜运行环境。

### 3.2. 第一版调用方式

#### 本机做种

```ts
import { createClient, fileSource } from '@q2q/client-web';

// 应用先完成文件选择。SDK 不显示文件选择器。
const client = await createClient({ trackUrl: 'https://track.example.com' });
const publication = client.publish({ source: fileSource(file) });
const unsubscribe = publication.subscribe(snapshot => render(snapshot));

// 等到种子准备完成且来源宣告成功，再向接收方分发种子。
const { seed } = await publication.ready();
saveSeed(seed.toBytes()); // 应用负责把字节保存成 .q2q 文件

```

用户点击“停止分享”时执行收尾，而不是导出种子后立即自动关闭：

```ts
await publication.close();
unsubscribe();
await client.close();
```

选择 `publish`，不另设同义的 `share`/`upload`：它对齐原生 SDK 的 Publication 概念，表示本机准备内容并提供给 Peer，不是上传到平台 Bucket。UI 仍可显示“分享文件”。平台托管上传是独立业务适配。

#### 预览种子并下载

```ts
import { createClient, fileSource, fileDestination } from '@q2q/client-web';

const client = await createClient({ trackUrl: 'https://track.example.com' });
const seed = await client.parseSeed(fileSource(seedFile));
showPreview(seed.info); // 此时不下载内容，也不发起 Peer 连接

// 在用户点击开始的事件中先申请保存位置，再调用 SDK。
const destination = await fileDestination(saveHandle); // 已由应用取得授权的句柄
const transfer = client.download({ seed, destination });
const unsubscribe = transfer.subscribe(snapshot => render(snapshot));

try {
  const result = await transfer.wait();
  showOutput(result.output); // 区分用户文件已保存和缓存仍待导出
} catch (error) {
  showError(error); // 结构化错误由应用翻译
} finally {
  unsubscribe();
}
```

`publish()` 和 `download()` 立即返回任务句柄，不等待哈希、发现或下载结束，应用因此能观察准备阶段并随时取消。参数形状错误或 Client 已关闭可同步抛错；开始后的异步失败进入任务终态，并由 `ready()` / `wait()` 返回。

#### 暂停、继续与刷新后恢复

```ts
import { createClient, browserState, fileDestination } from '@q2q/client-web';

const state = await browserState({ namespace: 'my-app' });
const client = await createClient({ state });
const transfer = client.download({ seed, destination });

await transfer.pause();  // 同一个活任务：停止调度，收敛在途写入
await transfer.resume(); // 继续同一个任务；不是重新创建 Client
await transfer.cancel(); // 保留检查点，结束这次运行
await client.close();
await state.close();
```

页面重新打开后，重新获取状态存储和目标权限。持久状态必须从首次下载起就启用，不能在刷新后追溯恢复未保存的数据：

```ts
const state = await browserState({ namespace: 'my-app' });
const client = await createClient({ state });
const checkpoints = await client.listCheckpoints();
const resumed = client.resumeDownload({
  checkpointId: selectedCheckpoint.id, // 用户选择的检查点
  seed, // 再次解析的原始种子；SDK 必须匹配检查点中的内容身份
  destination: await fileDestination(reselectedHandle)
});
await resumed.wait();
await client.close();
await state.close(); // 注入的存储由应用持有，Client 不擅自关闭共享存储
```

示例中的页面操作和变量来自应用，不是 SDK 隐式全局状态。首版默认使用内存状态；需要跨刷新恢复时显式提供浏览器持久状态存储。重开页面后的恢复会创建新任务句柄，检查点 ID 保持关联；任务 ID 标识本次运行，不冒充仍存活的旧任务。

### 3.3. Client 契约

以下 TypeScript 形状用于审查，类型细节以实际适配验证后的定义为准：

```ts
interface Client {
  readonly capabilities: ClientCapabilities;
  parseSeed(input: Uint8Array | ContentSource, options?: { signal?: AbortSignal }): Promise<Seed>;
  publish(options: PublishOptions): Publication;
  download(options: DownloadOptions): Transfer;
  resumeDownload(options: ResumeDownloadOptions): Transfer;
  listCheckpoints(): Promise<readonly CheckpointSummary[]>;
  discardCheckpoint(id: CheckpointId): Promise<void>;
  close(): Promise<void>;
}

interface ClientOptions {
  trackUrl?: string;
  state?: StateStore;
  limits?: Partial<ClientLimits>;
  sourceProvider?: SourceProvider;
}

interface Seed {
  readonly info: ContentInfo;
  toBytes(): Uint8Array;
}

interface PublishOptions {
  source: ContentSource;
  name?: string;
}

interface DownloadOptions {
  seed: Seed;
  destination: ContentDestination;
}

interface ResumeDownloadOptions extends DownloadOptions {
  checkpointId: CheckpointId;
}
```

- Client 在任务之间共享连接与资源预算，无全局单例。未执行网络任务时不强制连 Track；HTTP 来源足够时不以 Track 不可用作为下载的前置失败。
- `trackUrl` 显式配置此 Client 的 P2P 网络入口。省略时为 HTTP 来源下载模式；本机发布会明确报需要网络入口。种子中的 Track 候选可供应用预览后选择，不在首版隐式创建多个网络会话；主站自行注入官方网络配置。
- `parseSeed()` 只读取有大小边界的元数据并校验格式/内容身份，不访问种子中的来源；返回的 Seed 不可变，`toBytes()` 返回独立字节副本。外部传入对象仍不能绕过 SDK 的输入校验。
- 省略 `state` 时仍可暂停并继续当前标签页任务，但不承诺跨刷新恢复；能力值需反映实际 StateStore 与目标存储组合。
- `resumeDownload()` 与 `download()` 都返回 Transfer；前者在准备阶段校验原种子、检查点、目标身份和已保存内容，不按文件名和大小判断可恢复。
- 对同一个目标/检查点只允许一个写入任务；跨 Client/标签页也要由目标适配与版本化状态控制竞争，冲突返回 `DESTINATION_BUSY`。
- `discardCheckpoint()` 只删除恢复记录，不删除用户文件或云端内容；正在使用的检查点不能被清除。SDK 管理的缓存清理单独显式处理，不能借“清历史”隐式删除数据。
- 不增加 Client 级事件总线、任务历史 UI store 或另一套任务遍历订阅；首版由应用保存自己创建的句柄并逐任务订阅，SDK 内部仍登记资源用于关闭。

### 3.4. 任务句柄和观察

```ts
interface Transfer {
  readonly id: TaskId;
  snapshot(): Readonly<DownloadSnapshot>;
  subscribe(listener: (snapshot: Readonly<DownloadSnapshot>) => void): Unsubscribe;
  pause(): Promise<void>;
  resume(): Promise<void>;
  cancel(): Promise<void>;
  wait(): Promise<DownloadResult>;
}

interface Publication {
  readonly id: TaskId;
  snapshot(): Readonly<PublicationSnapshot>;
  subscribe(listener: (snapshot: Readonly<PublicationSnapshot>) => void): Unsubscribe;
  ready(): Promise<{ readonly seed: Seed }>;
  pause(): Promise<void>;
  resume(): Promise<void>;
  close(): Promise<void>;
}

type Unsubscribe = () => void;
type TaskId = string;
type CheckpointId = string;
type ContentId = `sha256:${string}`;
```

- `subscribe()` 立即提供当前快照，随后合并高频进度，默认约 100ms 一次；重要状态变化立即推送。慢观察者不积压每个块的事件，一个监听器异常不影响其他监听器或传输。
- `unsubscribe()` 可重复调用，不停止任务；JS 对象被垃圾回收也不作为可靠的停止机制。运行中的任务由 Client 持有，必须显式取消/关闭，或者关闭 Client。
- `ready()` 是首次种子准备和宣告成功的里程碑，不表示文件已送达，也不表示暂停后仍在线；当前状态看快照，恢复成功由 resume 确认。分享暂停再继续时仍使用相同内容身份。源文件变化或失去权限时停止宣告，不能带着旧种子继续服务新字节。
- `wait()` 不取消任务，不消费句柄；多个等待者可以取得同一结果。暂停时仍然等待；失败/取消后抛对应错误，成功时在最终快照之后返回结果。
- 不常驻暴露一个可能无人订阅的拒绝 Promise；只订阅进度的应用也不能因任务失败收到 SDK 产生的未处理 Promise rejection。
- 第一版不提供隐式的“失败任务重新开始”。失败是终态，有有效检查点时通过 `resumeDownload()` 建新句柄；分享失败后由应用重新选择/确认文件再发布。

### 3.5. 状态、进度与保存结果

| 下载状态 | 含义 |
| --- | --- |
| `preparing` | 打开目标、校验检查点等准备工作 |
| `waiting` | 等待合法来源或网络恢复，快照带受控的原因码 |
| `downloading` | 传输、校验和按块写入；阶段可在辅助字段进一步区分 |
| `pausing` / `paused` | 正在收敛在途操作 / 已停止调度，检查点已按能力提交 |
| `finalizing` | 收尾验证、刷新目标及提交结果 |
| `completed` | 内容已通过校验且目标完成约定的提交；是否已到用户文件由 output 区分 |
| `cancelling` / `cancelled` | 正在停止 / 已停止，保留可恢复数据，不假装已下载完成 |
| `failed` | 无法继续的错误，快照含结构化错误 |

Publication 使用 `preparing / publishing / pausing / paused / closing / closed / failed`；`publishing` 时用活动连接数区分等待接收与正在发送，不把无人连接当作失败。

```ts
interface DownloadSnapshot {
  readonly id: TaskId;
  readonly state: DownloadState;
  readonly content: ContentInfo;
  readonly receivedBytes: bigint;
  readonly verifiedBytes: bigint;
  readonly committedBytes: bigint;
  readonly bytesPerSecond: number;
  readonly checkpointId: CheckpointId | null;
  readonly error: Failure | null;
}

interface PublicationSnapshot {
  readonly id: TaskId;
  readonly state: PublicationState;
  readonly name: string | null;
  readonly totalBytes: bigint;
  readonly contentId: ContentId | null;
  readonly preparedBytes: bigint;
  readonly sentBytes: bigint;
  readonly bytesPerSecond: number;
  readonly activeConnections: number;
  readonly error: Failure | null;
}

interface ContentInfo {
  readonly id: ContentId;
  readonly name: string | null;
  readonly size: bigint;
  readonly blockSize: number;
  readonly blockCount: bigint;
}

interface DownloadResult {
  readonly content: ContentInfo;
  readonly output: OutputReceipt;
}

interface CheckpointSummary {
  readonly id: CheckpointId;
  readonly content: ContentInfo;
  readonly targetId: string;
  readonly committedBytes: bigint;
  readonly updatedAt: number; // Unix 毫秒
}

type OutputReceipt =
  | { readonly kind: 'user-file'; readonly targetId: string }
  | { readonly kind: 'staged'; readonly targetId: string; readonly needsExport: true };
```

- 字节数与文件偏移统一 bigint；块大小、连接数及瞬时速率是有边界的 number。公共快照不直接当作 JSON，跨 Worker 用无损传输，持久化按版本化编码。
- `receivedBytes` 是本次运行收到的数据，可能包含重传并超过文件大小；`verifiedBytes` 是当前已校验的去重内容，`committedBytes` 是其中已获目标提交确认的部分。恢复验证通过的旧块可计入后两者。主进度不直接使用收到字节除以大小。
- `sentBytes` 不表示接收方已校验、已保存或有人完整收到了文件。UI 的速度单位由应用格式化。
- `targetId` 是不可解析的引用，不是绝对文件路径或授权 URL。`user-file` 仅在用户目标写入和刷新确认后返回；OPFS 等缓存交付为 `staged`，界面显示待保存。应用提交给浏览器下载管理器后也不能凭调用成功就改称确认落盘。
- 下载完成不会自动做种。首版显式重新打开可读的已保存目标作为 source 再调用 `publish()`；对当前下载只写不可读的目标不显示继续分享。该步骤仍验证内容，不能把目标引用当作已经可信且不会变化的内存对象。
- 原始 Peer ID、认证材料、带授权参数的来源、文件路径和错误响应原文不进入默认任务快照；详细连接观测以后按实际需求增加，不先把内部对象暴露为公共 API。

### 3.6. 生命周期命令的精确定义

| 命令 | 成功返回时保证 | 不保证/边界 |
| --- | --- | --- |
| Transfer `pause()` | 不再新调度，在途操作停止或收敛，能提交的进度已提交，状态为 paused | 不等于释放保存的内容，也不保证跨刷新权限仍有效 |
| Transfer `resume()` | 已接受继续同一活任务的请求并恢复运行 | 不等于已连接到来源或已完成；只能从 paused 继续 |
| Transfer `cancel()` | 任务不再产生后续读写/状态提交，已释放任务占用的网络和写入资源 | 不删除用户目标；可恢复数据默认保留；wait 报 TASK_CANCELLED |
| Publication `pause()` | 不再接受新上传，撤销本次可用性，处理中的发送/读取已收敛，保留准备结果 | 撤销失败不能冒充成功；连接方的缓存不会被远程抹除 |
| Publication `resume()` | 已校验源仍匹配原内容并重新宣告成功 | 不表示已有接收方，也不自动同意分享内容变化后的另一个文件 |
| Publication `close()` | 本次发布关闭，读写资源释放，已尝试撤销；错误按契约报告 | 撤销远端失败仍须停止本地服务；远端旧记录可能等待 TTL 过期 |
| Client `close()` | 所有任务、连接、心跳和自有 Worker 已停止，恢复状态按契约提交；失败要报错且仍停止资源 | 不删除检查点、用户文件或平台内容；不关闭调用者注入的共享 StateStore |

同句柄的 pause/resume 依调用顺序处理；cancel/close 是终止请求，会阻止等待中的继续命令重新启动工作。异步读写带任务代次与资源租约，旧代次的迟到结果不能在取消或恢复后重新提交。

重复暂停已暂停任务、取消已取消任务、关闭已关闭资源可安全完成；已完成下载收到取消不回退 completed。对其他不合法的终态操作返回 `INVALID_STATE`。并发关闭返回同一次关闭的结果，不启动多次清理。关闭失败时本地资源仍须停止并在错误中说明远端撤销/提交未获确认，不能让重试 close 变成重新做种。

原生 `Transfer::cancel` 当前消费句柄并停止任务，Publication 通过 `close` 等待释放；这里的 JS pause/resume 与异步清理保证需要 host 编排或 Rust 共享能力补充，不能把原生 Drop 直接包装后就宣称这些保证已满足。

### 3.7. 存储、能力与扩展点

首版公有适配边界是 `ContentSource / ContentDestination / StateStore / SourceProvider`；Track/Punch/Peer 传输及 Worker 桥接先保持 SDK 内部 host 接口，通过确定性测试注入，避免过早发布内部调度类型。

Web/Node 存储与 WebRTC 都通过适配实现：Web 的 IndexedDB 主要存状态与恢复索引，文件字节优先写用户文件或 OPFS；Node 通过 napi-rs（Node-API） 复用原生 Rust 文件存储、检查点及 webrtc-rs。外部契约统一，内部不强求相同 JS 数据通道对象。具体对应及 Node 方案的待验证事项见 [宿主适配设计](#web-与-node-的适配对应)。

| 契约 | 必须明确的行为 |
| --- | --- |
| ContentSource | 元信息、范围读取、读会话关闭和源版本验证；描述符由应用持有，SDK 持有任务级读租约，结束后释放 |
| ContentDestination | 创建/恢复写会话、按偏移写入、提交、完成收据、独占写租约及释放；恢复绝不先清空目标 |
| StateStore | 命名空间、版本化读取、条件更新和列举/删除；检查点与目标分开提交，失败后重校验，不伪装跨存储原子事务 |
| SourceProvider | 为已确定的内容身份领取/刷新受限 HTTP 内容及证明来源；不能改变内容身份、移除校验或把平台账号塞进 Peer 协议 |

这些适配的精确低层类型在 S1 的下一步结合读写和 Worker 消息布局细化；本稿先确定面向使用者的稳定调用层。不把未定义的低层接口称为已完成。

Web 包已提供 `fileSource(file)` 和异步 `fileDestination(已授权句柄)`；`browserState({namespace})` 已提供版本化检查点，HTTP Client 已接线，P2P/发布仍在后续阶段。新下载默认不覆盖已有非空目标；覆盖需应用显式确认并在目标选项中允许。恢复按内容与检查点验证，而不是利用“允许覆盖”跳过身份检查。

ClientCapabilities 报告实际的 P2P、HTTP 下载与本地状态能力；目标是否能流式写入、读回、持久恢复则由具体目标适配报告。不得用单个 canResume=true 掩盖源文件、目标权限和检查点组合的限制，也不把“支持浏览器”泛化为支持任意文件大小。

ClientLimits 第一版对应原生的真实资源维度：下载 Peer 数、HTTP 请求数、上传连接数、并发读取数、缓存字节和上传速率；提供经过验证的默认值及范围，不允许无限制值绕开浏览器预算。默认值由能力验证确定，不以不存在的性能数据定稿。

### 3.8. 错误与验证清单

失败通过带 `code`、`retryable` 和受控原因字段的 `Failure` / `Q2QError` 表达，SDK 不输出本地化文案。首批需要区分：

- `INVALID_SEED`、`CONTENT_MISMATCH`、`SOURCE_CHANGED`；
- `NO_SOURCES`、`NETWORK_UNAVAILABLE`、`AUTH_REQUIRED`、`DISCOVERY_REQUIRED`；
- `PERMISSION_REQUIRED`、`STORAGE_FULL`、`DESTINATION_EXISTS`、`DESTINATION_BUSY`、`CHECKPOINT_MISMATCH`；
- `UNSUPPORTED_CAPABILITY`、`INVALID_STATE`、`CLIENT_CLOSED`、`TASK_CANCELLED`。

等待来源和正常重连不立即落为终态失败。`retryable` 只说明条件变化后可重试，不意味着 SDK无限重试或失败句柄可以直接 resume；首版遵循固定的等待、超时和退避预算。记录日志只用脱敏错误码，不拼入私钥、下载地址、种子原文或服务端原始错误。

实现前/实现中需要验证：

- [ ] API 示例能表达本地发布、种子预览、HTTP/P2P 下载、暂停/继续、取消、跨刷新恢复和完整关闭。
- [ ] 任务准备期间也能订阅和取消；subscribe 首个快照、退订、多个 wait 和未调用 wait 的错误处理没有歧义。
- [ ] cancel/close 与初始化、写盘、检查点提交、重新宣告交错时，旧工作不能污染后继状态。
- [ ] 暂停不阻塞其他任务，关闭 Publication 不误关 Client 或另一个下载；无全局单例和隐式网络配置。
- [ ] user-file 与 staged 输出各自有真实保存验收，浏览器权限缺失不会触发后台弹窗或假完成。
- [ ] Web 导入不拉入 Node 原生依赖，共享类型不依赖 File/DOM 私有对象，Node 包不在实现前发布。
- [ ] 协议和文件内容以共用 Rust fixtures 验证，保留既有原生公共 API。

本轮完成公共 API 首稿；低层适配器类型、Worker 命令/快照编码、精确资源默认值与 Rust 提取清单仍属于 S1/S2 的后续设计和验证，不将整个 SDK 设计或功能实现提前标记完成。

## 4. 多态与运行位置

### Web 与 Node 的适配对应

两端共用 Client/Transfer/Publication 契约、可信内容规则及协议，不要求使用同一种数据库或同一个 WebRTC 对象。存储进一步拆为文件字节与状态元数据：

| 职责 | Web | Node（napi-rs 路线，尚未实现） |
| --- | --- | --- |
| 文件来源 | File / 获得授权的文件句柄，按范围读取 | 本机文件系统，优先复用原生 Rust 文件读取及版本检查 |
| 下载字节与落盘 | 用户可见文件句柄；不具备条件时用 OPFS 缓存后导出 | 原生文件系统与已有 FileStore/DownloadStore |
| 任务元数据与恢复索引 | IndexedDB；不默认把大文件字节都塞进 IndexedDB | 版本化本地状态文件及原生检查点；出现实际查询需求后再评估 SQLite |
| Peer 数据传输 | 浏览器 RTCPeerConnection / RTCDataChannel | 优先复用 q2q-core 现有 webrtc-rs 传输，通过 napi-rs 绑定暴露 SDK |
| Punch 信令 | 浏览器 WebSocket，经 WSS 与现有认证帧接入 | 复用原生 QUIC/WSS 适配和票据/路由语义 |
| 调用入口 | `@q2q/client-web` | `@q2q/client-node` |

Node 已确定采用 napi-rs：JavaScript 调用层通过 Node-API 使用现有原生 Rust Client；文件读写、WebRTC 和数据面尽量留在 Rust，只跨边界传命令、元数据与合并进度，避免每个块都在 JS/Rust 间往返。Web 则使用 Rust 共享逻辑的 Wasm 绑定与浏览器 host。多态位于共同的宿主能力/核心逻辑边界，不把一整个 Native Client 假装成一个 JS PeerTransport，也不为了接口一致而重写现有 Node 传输核心。

napi-rs 技术路线已确认，具体绑定仍未实现。需验证 Tokio 与 Node 的调度、取消及异步关闭、回调退订与进程退出、结构化错误和 bigint 转换，并确定操作系统/架构支持矩阵与原生二进制分发。不依赖 JS 垃圾回收确认资源已释放，也不因 Node-API 的 ABI 稳定性假定所有平台都可用。

恢复契约一致不等于检查点可直接跨浏览器与 Node 搬运；两端目标标识、权限和存储格式可以不同，恢复仍要绑定原内容与目标并重新验证。IndexedDB 的事务也不能代替目标文件的提交。

参考：[浏览器 OPFS](https://developer.mozilla.org/en-US/docs/Web/API/File_System_API/Origin_private_file_system)、[Node-API](https://nodejs.org/api/n-api.html)。仓库原生实现见 `crates/q2q-core/src/webrtc_transport.rs`、`download_store.rs` 与 `destination.rs`。

### 浏览器 WSS 的证书前置条件

现有原生 Punch WSS 客户端通过自定义 rustls verifier 校验 Track 下发的 Ed25519 公钥；Punch 默认生成自签名证书；开启 `websocket_tls = true` 时 QUIC 与 WSS 共用该身份。`websocket_tls` 默认关闭，WebSocket 接收明文 WS，供反向代理终止公网 TLS；此开关不关闭 QUIC TLS。此路径不能直接推导为浏览器可连接：标准浏览器 WebSocket 不允许应用替换 TLS 校验器或读取服务器证书来做相同的公钥绑定。

Web 接入前必须提供域名匹配、有效且具有浏览器信任链的 WSS 入口。已确认采用独立 WSS 公网 TLS 终止（受信任证书），并保留原生 QUIC 的节点身份；发现契约应显式区分 WSS 地址与原生入口，不再默认把 quic:// 替换为 wss:// 就可用。若要求浏览器同样验证 Track 指定的节点身份，需要另外设计绑定会话的应用层签名证明，不能宣称现有 TLS pin 已在浏览器实现。首版由公信 WSS 证书认证代理入口，不另加 Punch 应用层签名证明；保留 Peer 签名与 DTLS 指纹绑定。实际代理部署和公网证书仍需接线验收。

- [ ] 检查目标 WSS 入口真实证书链、域名、有效期与端口，使用标准浏览器完成 TLS 握手和认证；原生 WSS 成功不能代替此验收。

### 适配职责

| 边界 | 最小职责 |
| --- | --- |
| 内容读取 | 获取长度、按范围读取、取消和释放；首个实现接浏览器 File |
| 下载写入 | 按偏移写入、提交、完成与释放；暴露实际持久能力和权限限制 |
| 检查点存储 | 按内容身份与目标存储绑定读写版本化恢复数据；不假定文件写入和 IndexedDB 是同一事务 |
| Track 控制传输 | HTTP 请求、预检兼容、超时与取消；签名及整数编码遵循同一协议 |
| Punch 信令 | WebSocket 连接、认证帧、请求/响应与重连，不承载文件字节 |
| Peer 数据通道 | WebRTC 消息收发、背压、关闭和错误；业务协议不散落在 DOM 回调里 |
| 授权来源解析 | 根据内容与权限领取或刷新短期来源，平台实现通过注入接入 |

这些是职责边界，不要求每个条目都新建公共类或文件夹。只对实际实现和测试需要的部分提供公共扩展点，内部细节保持扁平。

- 哈希、校验及可共享调度适合放到 Worker；浏览器对象由支持其 API 的宿主持有。不能假定 RTCPeerConnection、文件权限申请和所有存储 API 都能直接在同一种 Worker 中运行。
- SDK 封装宿主与 Worker 桥接。界面传命令、收快照，不收发 Peer 帧；字节尽量使用可转移缓冲，有界管理复制及队列。
- 主线程保留需要用户手势的权限申请，获得句柄后再交给 SDK。能力缺失通过检测与明确错误暴露，不让应用猜浏览器行为。

## 5. 任务、可信状态与资源契约

- 下载区分准备、等待来源、传输、校验、保存、完成和失败；分享区分准备、可提供、等待接收、分享和停止。暂停和取消的语义分别定义。
- 收到字节、内容校验、写入提交与允许做种分别确认。允许原位保存尚未校验的数据，但它们不能被标记为可信块或发送给其他 Peer。
- `pause` 停止新调度并收敛在途操作；`cancel` 终止当前任务并释放预算，保留/清除恢复数据必须由契约明确；`close` 关闭 Client 持有的任务、连接、计时器和 Worker。
- Client、任务、文件句柄和 Peer 连接的所有者明确；接口要说明调用者是否仍拥有传入句柄。重复取消/关闭、任务初始化失败和部分资源创建成功都能安全清理。
- 文件与检查点分别持久化，恢复时核对内容根、版本、目标存储、文件授权和已提交块；同名同大小不是恢复依据。
- 限制下载并发、上传连接、读取并发、上传速率、缓存和 DataChannel 队列；暂停任务不影响其他任务，单个下载方不能独占所有上传资源。
- 浏览器刷新/关闭后不承诺任务在线。恢复 API 恢复的是可校验状态，不是自动获得本机文件权限或后台常驻能力。
- 下载后做种是应用显式开启的能力，默认关闭；只共享已校验且满足提交条件的内容。

## 6. 实现前必须补齐的边界

本节是 2026-10-28 复审补充，具体实现和验收状态以 C1–C7 为准；未完成的网络和身份边界仍是后续依赖。S1/S2 必须先解决下列依赖，不能仅凭公共 API 示例进入完整客户端开发。

### 文件写入、提交与恢复

- 用户可见文件的 `createWritable()` 常通过临时文件写入，关闭流后才更新目标。`write()` 返回只表示写入会话接受数据，不能立即增加代表可恢复持久内容的 `committedBytes` 或推进恢复位图。暂停/取消需要明确选择关闭并提交，还是中止并丢弃本会话未提交写入。
- 存储适配应区分会话内已写入字节与已提交字节；重新打开写会话必须保留已有内容，避免默认截断。不能为每块关闭/重开整个文件而不评估整文件复制的时间和磁盘放大。
- OPFS 的同步访问句柄可在支持它的 Dedicated Worker 中按批次写入并 flush，再提交检查点。写文件与 IndexedDB 不构成原子事务：先确认字节提交、再更新索引；中断后只按真实可读内容重新校验恢复。
- 直接用户文件路径与 OPFS 暂存路径都要测强制关闭页面后的实际字节，记录暂停/取消后哪些数据保留。浏览器 API 的提交确认不承诺抵御硬件断电。
- bigint 是公共和协议表示；浏览器文件 API 接受 number 的长度/偏移必须先验证安全整数范围，超出能力的文件在开始前拒绝，不做有损转换。

依据：[createWritable](https://developer.mozilla.org/en-US/docs/Web/API/FileSystemFileHandle/createWritable)、[OPFS flush](https://developer.mozilla.org/en-US/docs/Web/API/FileSystemSyncAccessHandle/flush)。

### 暂存结果如何导出和清理

C3 已提供 opfsCache：应用保存 entry.id，页面重新打开后用 cache.open(id) 取得目标、完成状态和范围读取 source，再通过 entry.exportTo(已授权目标) 分块复制。导出可观察、可取消、可重试，失败及成功均保留原缓存；只有显式 remove(id) 才清理缓存。OutputReceipt.targetId 仍是存储目标引用，不自动代替缓存 ID 或下载检查点。

不得把整文件转为 Uint8Array 作为通用导出 API；只允许在经验证的文件上限内使用 Blob 降级。清除检查点、删除任务历史、释放 OPFS 内容是独立操作，清理活动目标应返回占用错误。当前实现通过 exportTo 和目标适配交付，不添加无法确认落盘的通用 save 承诺。

### 本机分享的种子、证明与内容身份

- `ContentInfo.id` 明确指 Q2Q `info_hash`（规范 info 字典的 SHA-256），不是文件的平面 SHA-256，也不是 Merkle root。新增独立 root 字段与种子来源预览字段时必须保持此区别，避免应用用文件摘要去查询 Track。当前 Seed.info 只有内容信息，尚不能满足选择种子内 Track 候选的描述，S1 要补不可变、脱敏的提示读取契约。
- 发布流程必须覆盖增量构建、证明存储与对端获取。小文件可内联叶子表；大文件外置证明时，需验证现有 Peer 元数据/证明请求如何从浏览器发布者获取，不能只导出 `.q2q` 后假设对端能校验。若协议缺少所需路径，列为先行改造，不默认上传到平台。
- 有界内存不仅约束文件字节，也约束叶子表、证明、候选 Peer 和块位图。检查现有 SeedBuilder 的内存规模，按文件/块数预算验证；不能仅因按块读 File 就宣称总内存不随文件增长。
- 来源变更检测按 File 快照及授权句柄实际能力定义；不能把名称、长度或最后修改时间当作内容一致的证明。

### WSS 发现、身份与 Peer 互通

- Punch v0.6.0 已发布 WS/TLS 开关，但没有实现浏览器 WSS 发现。Workers 控制面现保留 `quic://` 的 url，并沿签名注册凭据、Registry 发现与 Track 响应传递可选 websocket_url；原生发现/客户端维持既有行为，浏览器消费仍待接线。不能只在 UI 拼接代理域名。
- 已确认首版浏览器信任模型：公信证书认证代理域名，不新增 Punch 应用层节点证明。无论采用哪种 Punch 信任模型，都保留现有 Peer 身份与 DTLS fingerprint 绑定，不能以 WSS 成功替代 Peer 认证。
- 浏览器身份密钥由 SDK 生成并持有，应用无需自行管理 Peer 私钥。首版建议每个 Client 使用独立会话身份；持久恢复保存内容状态，不假装恢复旧在线身份。同一站点多 Client/标签页的身份与连接不得相互顶掉；若要持久 Peer 身份，需要另行确定唯一会话和重载语义。
- DataChannel 的消息长度与 Q2Q 帧边界需真实互通：按协商的 `maxMessageSize` 验证现有帧上限；超过时优先复用现有协议能力，不在 Web 单端私自切片后要求原生端猜测。验证 ordered/reliable 配置、首帧身份消息、背压和关闭竞态。
- Punch WSS 不等于文件能穿过 UDP 受限网络；直连、强制 TURN、TURN/TCP 或 TLS 分开记录。不能通过的网络标为不支持或明确降级。

依据：[DataChannel 消息上限](https://developer.mozilla.org/en-US/docs/Web/API/RTCSctpTransport/maxMessageSize)。持久 Peer 身份仍属后续设计；首版不新增 Punch 应用层证明，已确认的信任模型不等于已通过实际浏览器互通。

### 构建、部署与兼容矩阵

- SDK 提供默认 Wasm/Worker 加载，并允许应用指定打包产物地址；不默认依赖第三方 CDN，不在 SSR 导入时初始化浏览器全局。
- 验证 Wasm MIME、Worker 同源策略及 CSP 配置；首版不默认要求 SharedArrayBuffer 或全站 COOP/COEP，若采用多线程 Wasm 再评估嵌入和第三方来源影响。
- Chromium、Safari、Firefox 及手机实际填写能力矩阵：发布、P2P、HTTP、直接保存、暂存导出、刷新恢复、最大验证大小。不用一个“浏览器支持”布尔值掩盖不同路径。
- 种子或来源中的地址不自动附带平台 Cookie；认证与授权材料按目标来源隔离。跨来源跳转、Range 返回 200/206/416、Content-Range 和授权过期必须纳入来源适配测试。

## 7. 首版 SDK 验收

SDK 必须在没有 `apps/www` 和 Svelte 的最小浏览器页面中跑通，才能成为主站的依赖：

1. 浏览器 File → 增量种子构建 → 导出 `.q2q`，内存不会随文件长度无限增长。
2. 种子 → HTTP Range 来源 → 校验 → 真实保存结果；包含外置证明、错误内容、授权失效和无来源。
3. 浏览器 ↔ 浏览器与浏览器 ↔ 原生双向分享/下载；最终文件哈希一致，真实测试直连与 TURN。
4. 暂停、继续、取消、断网、重连、存储不足、权限撤销和 Client 关闭都能正确收敛及释放资源。
5. 恢复时拒绝错误文件、损坏检查点和错误内容根；不会分享未经验证或未提交的字节。
6. 多任务在同一个 Client 下正确共享资源；第三方页面可创建和销毁自己的 Client，不依赖全局应用单例。

目标浏览器、文件大小与降级策略以实测矩阵为准；初始化能力检测、不能支持的功能和错误码都能由调用者获得。平台配额与浏览器自身能力上限分别处理。

## 8. 执行项

### client-web 当前实施顺序

- [x] C1：建立独立工作区包、类型检查及测试入口；定义读写/提交初始契约，实现 File 范围读取、独立会话、取消与关闭。2026-10-28：类型检查、10 项 Node File/Blob 契约测试通过；不是浏览器写盘验收。
- [x] C2：实现文件句柄写入适配：乱序写、关闭提交、保留已有内容、取消/提交交错与独占目标。2026-10-28：10 项读取测试及 15 项 Chromium 存储测试通过；以真实 OPFS 文件句柄测试标准文件写入 API、跨句柄/标签页 Web Locks 和重载后的目标标识。系统保存窗口授权的外部文件及完整下载恢复仍留在 C5 验收。
- [x] C3：实现 OPFS 暂存、Dedicated Worker 同步写入/flush、完成缓存范围读取、流式导出及显式清理。2026-10-28：类型检查、10 项读取测试和 31 项 Chromium 存储测试通过，包含 Worker 原生短写/flush/close 故障、导出失败重试、活动删除保护、空闲 Worker 崩溃与页面刷新后缓存重开。系统文件选择、真实配额耗尽、大文件性能及下载检查点仍待 C4/C5。
- [x] C4：实现 IndexedDB 版本化检查点、租约/CAS、分页/关闭及目标绑定；补 Rust/Wasm 块校验和恢复会话。2026-10-28：类型检查、10 项读取测试、51 项 Chromium 存储测试、Wasm 真实绑定校验与 wasm32 Clippy 通过。覆盖字节/元数据提交中断、事务 abort、错误种子/目标、损坏/截断块、空文件、取消、跨标签页争用及刷新后重验；不表示网络下载流程完成。
- [x] C5（当前桌面范围）：迁入标准 Playwright Test；104 项通过、2 项 WebKit 不具备的 CDP 测量场景跳过，另有 10 项 Node 测试通过。完成 100 MiB/1 GiB 磁盘 profile、浏览器重启恢复、真实浏览器配额、原生系统保存与 Vite 构建验证。Firefox、Safari.app/移动端及真实文件权限撤销仍未验收，不能宣传为全面浏览器支持。
- [x] C6（HTTP 子集）：实现 Client/Transfer、Range 下载、Rust/Wasm 校验、受限来源刷新、暂停/取消及检查点恢复；真实 Chromium/WebKit 契约测试见下方记录。P2P/发布留在 C7。
- [ ] C7：贯通 WSS 发现/身份，接 Punch/WebRTC，完成浏览器与原生双向传输及生命周期验收。

按 C1–C7 依次实施，不等整个 SDK 完成才验证存储。下方 S1–S5 保留为完整设计和验收清单，C 项明确实际实施顺序；未勾选项不视为已实现。

### C2 实施记录

文件目标需要稳定标识才能跨句柄/标签页锁定，因此提前实现了一个内部 IndexedDB 目标映射表（随机 ID、句柄），通过短期 Web Lock 串行注册和 isSameEntry 比较；这不是 C4 的下载检查点。任务级独占 Web Lock 保持到 finish/abort，覆盖流关闭与惰性重开之间的间隔。记录清理与清站点数据后失效仍待 C4/C5。

`fileDestination` 工厂因此改为异步，调用示例统一使用 await。写入流开启时保留原文件；默认不覆盖非空目标。OPFS 句柄会得到 staged 收据，外部文件才得到 user-file。取消期间已启动的 close 可能完成，取消不会谎称能撤回提交。大文件反复重开流的复制成本尚未评估，不能把当前测试当作大文件性能验收。

### C3 实施记录

新增 opfsCache 工厂、OpfsEntry 及内部 Worker 桥接。缓存采用固定 SDK 命名空间、随机目录 ID、内容文件与版本化元数据；partial/complete 只说明存储状态，不替代内容校验。完成缓存只读，范围读取、导出、写入与删除共用目标锁。

同步 OPFS 写入不能回滚，因此修正 DestinationWriter.abort 的通用契约：停止任务并释放资源，不宣称所有适配都能抹掉未提交字节。用户文件流仍丢弃临时写入；OPFS 未完成内容必须在 C4 恢复时重新验证，当前不作为可共享来源。flush 成功只确认字节提交，不自动提交下载位图。

Worker 握手后才发送初始化，消息按请求 ID 匹配，复制并转移有界写入缓冲，处理原生短写与零进展；启动失败、原生 IO 失败、空闲崩溃均能清理。导出按块等待目标写入完成，保留源缓存并返回目标真实收据。已开始的宿主完成标记提交无法强行撤回，取消结果不冒充完成。

当前验证使用独立 Chromium 148 的真实 OPFS/Worker/IndexedDB/Web Locks；未使用系统保存窗口，未验证 Safari/Firefox、大文件峰值与真实磁盘耗尽。刷新测试只接续固定已知数据，不是已经实现内容身份与位图的下载恢复。公开用法及边界集中在包 README，不另建实验说明。

### C4 实施记录

StateStore 使用独立版本化 IndexedDB 表，按 namespace/id 定位；记录白名单字段，不持久化原始带来源的种子和凭据。位图每次读出都复制，提交字节数派生计算；读取与列表只是持久提示，可信运行进度必须来自恢复会话。检查点租约跨标签页独占，条件比较与写入在同一事务完成，活动记录不能删除。

新增 Rust/Wasm BlockVerifier，验证种子、外置/内联证明根及块长度/哈希。JS 只做宿主编排，不复制 Merkle 算法。恢复先核对内容与目标，再逐块读回登记内容；成功块写回当前写入会话并提交，损坏/缺失位清零，再推进修订号。未登记裸字节不增加可信进度。上层 commit 先提交目标、再更新位图；第二步失败保留旧记录，下次重验。

DestinationWriter 增加仅用于恢复的原始范围读取（不存在返回 null），不是可共享来源。OPFS 显式 resume 可撤回已完成标记并重验，普通 source 仍不能读取 partial 缓存。取消/关闭等待在途工作收敛，旧工作不能在返回后继续更新。StateStore 的生命周期只涵盖数据库和检查点租约，应用须先关闭文件会话，再关 state。

C4 当时提供的是存储会话 resumeCheckpoint，Client.resumeDownload 后于 C6 接线；网络调度、资源预算的完整协调及 UI 状态机留在 C6/C7。当前证明预算 16 MiB、最多 524,288 块；真实大文件、Safari/Firefox 和系统保存窗口仍待 C5。

### C5 桌面验收记录（2026-10-28）

浏览器自动化已从独立 playwright-core 入口迁到标准 `@playwright/test`：`playwright.config.mjs` 注册项目、隔离 profile、管理超时与报告；共用用例、打包和大文件测试都由 runner 调度。104 项通过、2 项有理由跳过，不把跳过计为通过。Node 的 10 项范围读取测试独立保留。

| 环境 | 实测结果 | 支持边界 |
| --- | --- | --- |
| Chromium 148.0.7778.96 / macOS 正常磁盘 profile | 共用契约、CDP 配额/清理、100 MiB/1 GiB 重启恢复与导出通过 | 仅是 SDK 存储验收，网络下载尚未接线 |
| Chromium 原生系统保存窗口 | 65,553 字节乱序写入，user-file 收据；Node 独立读回字节与 SHA-256 一致 | 实际用户文件已验收；真实撤销授权仍未验收 |
| Playwright WebKit 26.0 | 共用存储契约和 Vite 产物通过 | 不是 Safari.app/iOS 验收；没有 showSaveFilePicker，CDP 专用两项明确跳过 |
| Firefox 144.0.2 / 本机 | headless 与可见窗口均未完成启动，子进程 sandbox extension/图形初始化报错 | 环境阻塞，不推断 SDK 不支持，不关闭浏览器安全保护绕过 |
| Android / iOS / 本机 Safari.app | 未执行 | 不宣传支持范围与文件大小上限 |

兼容性修复：WebKit 不支持把文件句柄存进 IndexedDB 或传给 Worker。OPFS 改为对其逻辑相对路径派生稳定不透明 ID，Worker 在内部按 SDK 私有路径打开同步句柄；外部用户文件仍用 isSameEntry 和句柄映射。路径摘要仅用于宿主目标定位，不是内容身份或第二套 Merkle 算法。此 SDK 尚未发布；此前实验性 OPFS 随机目标 ID 与新 ID 不同，旧实验检查点应明确报目标不匹配，不静默继承位图。

生命周期修复：关闭/失效的检查点租约发出 AbortSignal，恢复会话据此停止底层文件 IO。浏览器直接清除站点数据不保证先通知 SDK，活动同步句柄可能令清理等待；应用入口应先停止任务，再清理。测试覆盖状态关闭主动释放空闲 Worker，之后清站点数据使旧记录与缓存失效，不能把它描述为任意外部清理都可无条件自动恢复。

磁盘 profile 测量（标准 runner 当轮，500ms RSS 采样）：

| 文件 | 浏览器初始 RSS 求和 | 采样峰值 RSS 求和 | 缓存 + 导出后的 origin 用量 | 清理后 origin 用量 |
| --- | --- | --- | --- | --- |
| 100 MiB | 317 MiB | 599 MiB | 约 200.02 MiB | 约 21.6 KiB |
| 1 GiB | 332 MiB | 738 MiB | 约 2048.18 MiB | 约 187.0 KiB |

文件按块生成、写入与校验，提交四分之一后关闭浏览器，重开同一临时磁盘 profile 并重验登记块，再写完、读回并导出；缓存与导出内容均匹配原种子。导出目标是第二个 OPFS 文件，系统文件保存另有原生窗口测试。RSS 对浏览器进程树求和，含 Worker、Wasm、原生缓冲及重复计数的共享页；500ms 采样可能漏短峰，不是精确物理内存或上限承诺，也不能用两组样例证明任意大小恒定内存。

Chromium 将测试 origin 配额限制为 8 MiB 后，写入约 4 MiB 即按宿主返回 STORAGE_FULL，条目保持 partial；测试后解除覆盖并清理。没有通过填满真实磁盘验证磁盘耗尽。早先隐身上下文中 1 GiB 曾失败，正常磁盘 profile 则通过；不将本轮结果扩展为隐身模式容量承诺。

Vite 8.3.4 对真实 @q2q/client-web 包导出构建，Chromium/WebKit 均在同源 Worker/脚本及 wasm-unsafe-eval 的 CSP 下完成校验、检查点、写入及导出。报告保存在本地 `.tmp/client-web-playwright/`；测试说明统一在包 README，不再建立平行设计文档。

后续扩展验收（不计为已通过）：

- [ ] 在可正常启动的 Firefox、真实 Safari.app 和移动设备验证能力与保存降级。
- [ ] 覆盖用户真实撤销文件授权，以及更多浏览器/磁盘环境；权限拒绝分支目前有故障注入测试。
- [ ] 单独测量用户文件反复关闭/重开、频繁批次提交的放大成本；1 GiB 连续导出不代替此项。

### C6 HTTP 子集实施记录（2026-10-28）

新增 createClient、Seed 与 Transfer。源码类型为当前真实 HTTP 子集；上方含发布、Track 和 P2P 的 API 仍属完整设计提案，不表示全部实现。SSR 导入不初始化浏览器资源，Wasm 延迟加载；parseSeed 只解析和复制受限种子，不访问其地址。HTTP 使用浏览器 Fetch，证明根和块内容仍由 Rust/Wasm 校验，没有再写 JS Merkle 算法。

请求共享 Client 并发预算，元数据、响应体及块缓冲有界；精确检查 Range、长度和响应，单块全文件可接受 200，多块忽略 Range 明确失败。外置证明先校验，错误来源可切换；受限地址由 SourceProvider 领取，并发未授权只刷新一次。快照与结构化错误不携带凭据 URL。浏览器不能检查每个中间重定向，未实现顺序整文件降级或 HTTP 大文件性能验收。

任务区分收到、验证、提交与最终完成。暂停中止在途网络，等待写入及校验块提交，再释放租约；恢复重读目标重新校验。取消等待资源关闭、保留可恢复记录，不虚报已保存字节可撤回。默认内存状态仅当前 Client 可用，刷新恢复显式注入 browserState；Client 不关闭借入的状态库。观察者异常隔离，多次 wait 共享结果，终止任务不保留在 Client 活动集合。

本轮实际验证：HTTP 的 64 项 Playwright 用例全部通过（Chromium 148 / WebKit 26，含两个真实页面刷新恢复用例）；原有存储与 Vite 8.3.4 打包回归 103 项通过、1 项 WebKit CDP 专用用例跳过；10 项 Node File/Blob 测试、TypeScript 检查与 git diff --check 通过。大文件存储测量沿用 C5，本轮未重复，也未据此宣称 HTTP 大文件压测通过。

实际调用、默认预算、错误与取消边界集中在 [包 README](../packages/client-web/README.md#http-下载任务)。测试覆盖本地 Range/CORS 服务、真实 OPFS、外置证明、损坏来源修复、授权刷新、并发预算、超时/重试、暂停/取消、目标隔离和刷新恢复；浏览器版本与未验收边界沿用 C5。

### C7 接线顺序与待确认决策

2026-10-28 核对实际代码：discoveryNodeSchema / Registry 注册只接受 quic 地址；Track PunchWire 只有 punch_id/url/public_key；原生 WSS 客户端从 quic 地址派生同端口 wss，并用节点公钥校验证书。浏览器不能继承此校验方式，不能直接把相同 URL 转换逻辑复制过去。

实施拆分如下，未验证前均不勾选：

- [x] C7.1：维护者确认首版采用受信任 WSS 域名认证代理入口，保留现有 Peer 签名与 DTLS 指纹绑定；不另加 Punch 应用层会话签名证明，不宣称浏览器实现原生证书公钥绑定。
- [x] C7.2（Workers 控制面）：注册凭据可带 websocket_url，D1 存储/只读 RPC、Track 分配/候选与会话一致性贯通；保持旧 QUIC 字段及缺省响应。地址属于签名身份，更新/移除均需提高 identity_version，DO 旧账本增列并保留防回滚记录。浏览器消费及真实公网证书验收仍留后续项，不表示 P2P 已接通。
- [x] C7.3（控制面）：Wasm 复用现有 Peer ID、票据、Join 签名、Track/Punch MAC；浏览器提供随机数与宿主 IO。每个 Client 独立临时身份，显式重连沿用本 Client 身份；数据面身份 transcript 随 C7.5 实施，持久身份未实现。
- [x] C7.4：实现有界且可取消的 Track 控制请求、Punch WebSocket RPC/绑定心跳；覆盖 Join 认证、关闭、超时、迟到响应及显式重连，票据和密钥不进入公开快照。公网 WSS 与数据面首帧认证仍待后续验收。
- [ ] C7.5：浏览器双通道及身份绑定已实现，Chromium ↔ 原生双角色帧互通通过；WebKit ↔ 原生 ICE 尚待解决，保留未完成状态。控制通道可靠有序，数据通道保持原生 negotiated stream 1 / unordered / maxRetransmits=0；不单端私造切片协议。
- [ ] C7.6：下载/做种复用现有已校验提交块及存储生命周期，完成浏览器 ↔ 原生双向字节校验；直连、强制 TURN 与 UDP 受限网络分开验收。Playwright 负责浏览器契约和互通测试，真实公网 WSS 证书另行实测。

本轮验证：Registry 30 项、Track 79 项、平台节点视图 3 项测试通过，Registry/Track 类型检查及 git diff --check 通过。补加 WSS-only 地址变更导致旧会话失效的 workerd 断言后，6 项状态/HTTP 集成测试再次通过。没有执行公网 WSS 握手或浏览器/原生 P2P 验收。

C7.2 新增 Registry D1 0004_punch_websocket.sql；部署前需要应用迁移，CI 校验新列存在才部署，不自动修改线上数据库。WSS 入口来自签名凭据，不从未签名心跳或 UI 输入覆盖；现有原生客户端继续使用 QUIC/原有 WSS 路径。当时浏览器身份适配及 P2P 尚未实现，控制面后续进展见下节；未修改线上服务。

### C7.3/C7.4 浏览器控制面验证（2026-10-29）

新增 Client.connectNetwork / NetworkSession，按显式 Track URL 建立连接，当前不由主站自动启用。Rust 协议库增加无服务运行时的编译边界，Wasm 复用实际 Join/票据与 MAC 编码；u64 序列保持 BigInt/原始 JSON，不经 JS 浮点数往返。私钥留在 Wasm，关闭 Client 释放；绑定和会话密钥不进入公开快照。

Track announce 可指定 punch_transport，浏览器只分配有 websocket_url 且 WebSocket 容量未满的节点；旧请求保持原有规则。公开 JSON preflight 不消耗 Peer/IP 限流。公网继续要求 HTTPS/WSS；仅 HTTP loopback 页面允许本地 HTTP/WS 地址，Registry 也只为 loopback WS 提供测试例外。

控制请求/帧最大 64 KiB，最多 32 个待响应 RPC、8 个在途推送处理，发送缓冲预算 256 KiB。关闭中止网络、排空心跳并尽力发送签名 exit；Track 未提供删除会话接口，记录按 TTL 到期。断线转 offline 并停止心跳，不自动无限重试；调用者显式 reconnect，关闭 Client 后不能再连接。尚未提供 RTC 的 SDK 拒绝未请求的连接。

本轮 Chromium/WebKit 共 20 项控制面测试通过：临时真实 workerd Track/Registry 与原生 Punch 完成认证、合并心跳、双 Client 隔离、重连、断线、自动心跳与清理；双浏览器 Peer 经原生 Punch 完成 connect/accept/offer/answer/candidate/cancel 信令（测试 SDP，不代表 RTC 协商）。另有故障注入验证超时、取消、迟到响应、RPC 上限、异常文本/超大/无效帧。Wasm 以原生 Track fixtures、Node 独立 Ed25519/HMAC 对照，包括 u64 上界。

本轮其余回归：SDK 存储/HTTP 165 项通过、1 项 WebKit CDP 用例跳过；打包脚本修复 Vite ESM 入口后，Vite 8.3.5 的 2 项 Chromium/WebKit 测试通过。主站生产预览 12 项、Node File 10 项、Registry 30 项、Track 81 项、Rust Punch/Track 55 项通过；Wasm clippy（warnings 为错误）、SDK/Track 类型检查、主站检查与构建、9 项 CI 门禁测试通过。未重复 C5 的 1 GiB 测量。

CI 增加控制面测试及 Wasm 对照；依赖门禁跟踪 Wasm 的 Rust 依赖闭包和浏览器集成测试所用 Track/Registry。未测试公网代理证书、浏览器↔原生 RTC 字节、强制 TURN 或跨 NAT，不提前开放 p2p/publish 能力标记。

### C7.5 WebRTC 传输层进展（2026-10-29）

新增 SDK 内部 rtc-peer.ts，尚未从 Client/主站开放 P2P。浏览器 RTCPeerConnection 接 offer/answer、早到候选的有界暂存与串行应用；q2q-control 为可靠有序通道，q2q-data 与原生一样固定 negotiated ID 1、无序、maxRetransmits=0。没有更改原生协议或引入 Web 单端切片。控制帧最多 16 KiB，二进制 Piece 仍为 17 字节头 + 最多 1024 字节；低于现有控制帧上限的 SCTP maxMessageSize 明确拒绝。

身份帧与 SDP SHA-256 指纹解析提取到 primitives.peer_auth，原生 SDK 与 Wasm 共用。Wasm 使用双方新挑战、信令角色、双方公钥、内容 ID、连接 ID 和实际本地/远程 SDP 指纹生成原有 transcript。严格先 hello 后 proof，失败后不能复活；认证前不暴露业务消息。JS 只负责浏览器宿主 IO，控制消息保持原始 JSON，块序号使用 BigInt；不经 number 重写 u64。

发送最多 8 个待处理任务、256 KiB 宿主缓冲/待发载荷预算，bufferedamountlow 唤醒、取消/关闭清理；控制和数据收件箱各有 256 条/256 KiB 上限，溢出关闭连接。关闭阻止后续发送、清理身份交换/计时器并排空已注册宿主工作。未完成块校验、重传调度、Track 文件发现或分享发布接线，capabilities.p2p/publish 保持 false。

本轮验收：

- Chromium 148 / WebKit 26 浏览器双端认证、双向控制/二进制帧交换、内容/连接/公钥错配、取消和超时测试通过。
- 背压/取消/关闭与小 maxMessageSize 通过宿主故障注入验证；收件箱溢出使用真实 RTC 消息触发。
- Chromium ↔ 实际 webrtc-rs（原生 SDK 的 negotiate + PeerIdentity）在两种发起角色下认证并双向回传控制帧与 1024 字节 Piece，通过 u64::MAX 序号和逐字节比较。TCP 桥仅负责本地测试信令，内容走真实 WebRTC；此次未将 Track/Punch 连入同一个下载链路。
- RTC 共 20 项通过，2 项 WebKit/native 默认跳过：macOS Playwright WebKit 收到原生 host 候选后仍停留 ICE new，没有候选对/数据通道；具体原因未确认。尝试仅 loopback 候选及按 SDP MID 应用候选未解决，不认定为已兼容。`TEST_NATIVE_WEBKIT=1 npm run test:rtc --workspace @q2q/client-web` 可显式重跑两种角色；默认 CI 保留 WebKit 浏览器双端契约与 Chromium 原生互通。
- Wasm 用 Node 独立校验完整身份 transcript 签名，对角色、内容、连接、密钥和指纹变化验证不能重放，并覆盖身份帧乱序、重复字段及线协议长度。
- 原生 core 153 项通过、4 项 opt-in/基准用例忽略；新桥由 Playwright 显式运行。primitives 113 项通过。Wasm clippy、SDK 类型检查通过。

后续继续定位 WebKit/native ICE，并推进 C7.6 的 Punch 信令路由、块调度与真实文件校验提交；不把当前 Chromium 结果扩展为 WebKit 支持承诺。公网 WSS、跨 NAT、强制 TURN 与浏览器↔原生完整文件传输仍未验收。

### C7.6 接线拆分

- [x] C7.6a：已接 Client 下载：显式 Track 发现、目标 Punch WSS 协商、接受票据校验、逐块接收/丢包修复、已验证存储提交后 ACK；复用内嵌叶子表或已有 HTTP 外置证明校验，不声称只有 Peer proof 的外置种子已可恢复。
- [x] C7.6b：Peer 分块证明根绑定、紧凑见证与原子检查点更新；外置种子无 HTTP 证明源的下载和真实刷新恢复通过。
- [ ] C7.6c：Web 发布、部分块共享和主站接线。
  - [x] C7.6c.1：独立 Web SDK 完整文件 publish/Publication，浏览器及原生下载与生命周期本地验收（2026-11-08）。
  - [x] C7.6c.2：下载中的已验证提交块共享及动态可用性。2026-11-08 downloadShared / resumeDownloadShared 与活跃写入会话绑定的本地验收通过；不含完整三端调度/完成后长期做种，边界见下方记录。
  - [x] C7.6c.3：主站分享和 P2P 任务接线及实际页面本地验收。2026-11-08 生产预览 22 通过/2 WebKit-native 跳过；跨设备/公网 UI/TURN 与上线仍未完成。
- [ ] C7.6d：公网 WSS、跨 NAT、强制 TURN 与 WebKit/native 兼容验收。

### C7.6a 下载接线与验收（2026-10-29）

Client 的显式 trackUrl 现在启用 P2P 下载尝试；省略时仍为 HTTP 模式，不访问种子内 Track 地址。发布能力保持 false，apps/www 尚未注入入口或开放 P2P UI。任务复用现有 Transfer、BlockVerifier、CheckpointSession 和目标存储，没有另建页面下载状态机。

新增 peer-dial 负责独立 Punch WebSocket、connect/accept/offer/answer/candidate 路由，Wasm 校验接受票据的签名、双方身份、内容、连接、受众与时效。Track 查询与心跳共用序列化 MAC 计数器；关闭拒绝新查询/dial，排空已登记查询/建连/Peer 并清除凭据。Peer 连接默认全 Client 2 个，可配置 1–16，独立于 HTTP 请求额度；每个连接维持一个块请求窗口。

块拼装、重复冲突、缺失区间与位图解码提取到 primitives.peer_wire，原生 SDK 和 Wasm 共用；保留原生错误类别。网络位图 MSB-first 与浏览器检查点 LSB-first 的转换留在 Wasm。Peer 控制帧里的 block_index 以字符串传给 JS，不损失 u64 精度。

接收循环在磁盘提交期间继续排空重复包；PieceNack/RequestProof 修复丢失数据。对字节完成可信校验、写入并提交目标及检查点后才发送 Have/BlockDone，不能以收到字节代替提交。暂停/取消关闭本任务连接，保留已提交块；继续/另建恢复任务重读重验旧检查点。wait 与 completed 在本任务 Peer 清理后才结算，Client.close 不会遗漏尚在关闭的任务。

本轮实际验证：

- 9 项 Chromium 真正端到端用例通过：临时真实 Workers Track/Registry + 原生 Punch + 原生公开 SDK publication；浏览器用公开 Client API 下载。没有 TCP 信令桥；基础用例不提供 CDN 内容来源，HTTP 回退用例显式注入本地修复来源。内容经过真实 WebRTC，786,449 字节文件的 OPFS 导出逐字节及独立 SHA-256 一致。
- 内嵌叶子表下载、暂停/继续、丢包后真实原生重传、证明丢失后补取、坏块不进入检查点、坏 Peer 后 HTTP 修复、取消再恢复、多个任务共享一个连接名额时取消其中一个不影响另一个，均通过。丢包/损坏由收包接口注入，不冒充真实 UDP 网络劣化。
- 外置种子通过 HTTP 取得叶子表、数据全部从 Peer 获取的组合通过；没有 HTTP 证明源、仅 Peer 分块 proof 的外置种子仍未支持（C7.6b）。
- HTTP/网络/P2P 合并回归 92 项通过、8 项 WebKit/native P2P 跳过；另增关闭并发签名查询的 Chromium/WebKit 两例、证明补取一例通过。RTC 20 项、HTTP/控制面既有契约先前本轮回归通过，2 项 WebKit/native RTC 仍跳过。
- core 153 项通过、5 项 opt-in/基准忽略，新 publication helper 由浏览器套件显式运行；primitives 113 项、Node/Wasm 独立签名/位序/块拼装对照通过。SDK 类型、core/Wasm clippy、主站类型及构建通过，主站生产预览 12 项通过。
- CI 增加 test:p2p；本轮未执行线上部署、数据库迁移或创建 PR。

范围：Peer 最多 65,536 块，候选每次查询最多四页、初次为空短暂重查三次、每轮依次尝试最多八个候选，之后退到现有 HTTP 来源。不是原生完整的来源竞争策略；P2P 每块提交而非批次提交，未测大文件性能。未验证公网 WSS、强制 TURN、跨 NAT 或真实 Safari/iOS，WebKit/native ICE 仍未解决。C7.6b/c/d 保持未完成。

### C7.6b 纯 Peer 证明持久恢复（2026-10-29）

不再为纯 Peer 外置种子预取完整叶子表。HTTP 表不可用且有显式 Peer 网络时建立种子根绑定的稀疏验证器，保留原始 Proof 帧给 Rust，避免 JS 解析损失节点的 u64 位置。验证实际块字节后，将证明规范化为紧凑兄弟哈希见证；有效的非最小兄弟表示也可规范化，不改变现有 Peer 线协议。

审查同时修复 primitives.merkle.verify_range 的路径覆盖问题：证明节点不能替换当前块或其祖先，越界、重复位置及范围溢出明确拒绝。新增 root-only、覆盖叶/祖先、重复和越界反例，原生与 Wasm 共用修复；未声称已发布给线上客户端。

持久格式与自定义适配契约集中在 [包 README](../packages/client-web/README.md#纯-peer-证明的恢复状态)。浏览器数据库原地升级到版本 2，保留旧检查点记录并增加独立见证表；默认内存实现也支持。目标先提交，见证与位图 CAS 再在同一事务更新。配额失败不会留下仅位图成功的记录。恢复每次仅读取一块的本地见证，用重新提供的种子根重验实际字节，内置存储不在重验时向 Track/Punch/CDN 补取证明；缺失或损坏则撤回相应位与见证，后续重下。

本轮本地验证：

- 完整 P2P 套件 Chromium 17 项通过，WebKit/native 17 项按已知 ICE 限制跳过；涵盖纯 Peer 数据/证明、坏证明/坏字节拒绝、默认内存状态，以及页面实际 reload 后正常保留或文件/证明/证明缺失导致撤回重下。测试使用真实本地 Track/Punch 与原生 SDK publication，未提供 HTTP 证明地址。
- 存储/HTTP 回归 177 项通过、1 项 WebKit CDP 专用跳过；另有两项旧适配器兼容检查通过。见证专项覆盖复制、命名空间、CAS、删除、目标提交后证明写入配额失败时的事务回滚、未来版本与版本 1 数据库升级。
- Node/Wasm 以独立 SHA-256 树验证 1/3/5 块、奇数层提升、展开证明规范化、跨索引/坏字节/路径覆盖反例；Node File/内存状态单测 11 项通过。
- primitives 114 项、core 153 项通过（5 项 opt-in/基准忽略），原生公开 SDK 11 项通过；core/Wasm clippy、SDK 类型检查通过。主站检查/构建及生产预览 12 项、Vite/CSP 打包 2 项通过。

限制：WebKit/native ICE 未解决，公网 WSS/TURN/跨 NAT 未验收；未重复 C5 大文件容量测量，也未测大量见证的配额和恢复性能。纯证明模式不使用无对应证明的 HTTP 内容回退；有完整叶子表的原路径保持原有回退。下一项是 C7.6c Web 发布/部分块共享及主站接线。未部署、未创建 PR。

### S1. 接口与生命周期设计

- [x] 对照原生 Client/Transfer/Publication 写出公共 API 首稿与主要调用示例，见[公共接口草案](#3-公共接口草案)。
- [ ] 对照原生 Client/Transfer/Publication 和首版用户路径，确定 Web SDK 公共类型、句柄、命令与快照。
- [ ] 给出来源、目标存储、检查点和网络适配契约，以及资源归属、取消、恢复和错误语义。
- [ ] 明确 Browser host / Worker / Rust/Wasm 的调用关系与消息协议，决定最小 Rust 提取范围。
- [ ] 写出最小的本地分享、下载、进度订阅、恢复和关闭调用示例；审查第三方能否脱离主站使用。

验收：主要路径能用接口草案表达，失败与生命周期也有定义；没有靠界面内部状态补齐 SDK 缺失的行为。当前文档只是设计起点，不提前将此项勾为完成。

### S2. 存储实现与能力验证

先建立 `packages/client-web` 的真实存储模块和自动化测试，再用最小浏览器页面验证；不写一套随后丢弃的实验存储。存储测通后再接 HTTP、Punch/WebRTC。结果回填本文件，不另建平行设计稿：

1. **文件实验**：乱序写入、暂停提交、强制刷新、恢复校验，以及 OPFS 导出失败与重试；确定存储适配和提交语义。
2. **连接实验**：浏览器经受信任 WSS 入口认证 Punch，完成浏览器与原生的 Peer 身份绑定及小文件双向互通；确定发现契约与信任模型。
3. **完整性与规模实验**：验证大文件外置证明可获取、帧长度和有界队列，以及 100 MiB/1 GiB 的真实内存与磁盘用量。

- [ ] 先用最小页面验证浏览器文件与保存路径、Wasm 共享逻辑、Track/Punch/WebRTC 认证与实际字节互通。
- [ ] 验证 Window/Worker 可用性、背压、CORS、证书、协议无损整数和受限来源授权。
- [ ] 记录浏览器、大小、内存、落盘及原生双向互通矩阵，修订不能成立的接口假设。

验收：接口的关键依赖可行，尚不成立的能力有明确调整或降级方案。

### S3. 共享内核与 HTTP 下载

- [ ] 完善 S2 建立的 `packages/client-web`：明确初始化、Wasm/Worker 资源加载、类型导出与构建产物；SSR 导入不立即访问浏览器全局或启动任务。
- [ ] 在已测通存储之上接 HTTP Range、真实校验、暂停及恢复，再推进 P2P；不等整个 SDK 完成才测试存储。
- [ ] 实施已确认的 Rust 共享提取和最小 Wasm 导出，保留原生 SDK 公共行为。
- [ ] 建立内容、认证及 Peer 协议共用 fixtures；以桩存储、桩网络和可控时钟测试时序及故障。

### S4. 浏览器 P2P 传输适配

- [ ] 接好 File、下载目标、检查点、Fetch、WebSocket 和 WebRTC，完成 Worker 桥接及有界资源控制。
- [ ] 完成 HTTP 与 P2P 分享/下载，接授权刷新和真实保存/恢复状态。
- [ ] 按首版 SDK 验收逐项测试，并回归原生 SDK 受影响的行为。

### S5. 独立示例、检查与接入

- [ ] 提供不依赖 Svelte 的最小分享/下载示例，验证第三方打包器加载 Wasm/Worker 的方式。
- [ ] 补单测、真实浏览器集成测试和原生互通检查；CI 按依赖触发 SDK 与主站检查。
- [ ] 同步 API、支持矩阵、取消/恢复语义及已知限制，再由 `apps/www` 消费 SDK。

### Node 后续适配（Web 存储优先）

- [ ] 建立 `packages/client-node`，使用 napi-rs 绑定 q2q-core，复用 Rust 文件、检查点、WebRTC 和连接预算。
- [ ] 验证 JS 命令到原生任务、进度订阅、显式 cancel/close、异常及进程退出时的资源清理。
- [ ] 按实际复用提取有具体职责的公共包，运行两端共同契约测试；确定原生构建矩阵和发布方式。

主站原型可以消费这个契约的模拟实现来确认交互，但不能据模拟进度宣称 SDK 已完成。SDK 的版本、发布渠道和对外稳定承诺待接口与兼容性通过后确定，不在设计阶段发布空包。

### C7.E：Punch 端点列表统一（2026-10-29 计划，2026-11-07 E1 已完成并接线，尚未上线）

维护者确认 MVP 全栈一次性切换，不保留旧字段、旧客户端协议或旧数据库读取路径。统一使用 `endpoints: string[]`，URL scheme 决定连接器，不再额外存 transport 类型。此决策取代前述 C7.2 的单 url + websocket_url 和增量兼容方案；此前测试记录仍是旧实现的历史结果，不代表本节已完成。先完成本节，再继续 C7.6c Web 发布。

目标示例：`{"punch_id":"hk-1","endpoints":["quic://punch.example.com:4433","wss://punch.example.com/ws"],"public_key":"…"}`。

#### 契约与边界

- Punch 的签名凭据、发现、分配、候选和管理接口都使用同一个端点列表，不再返回单值 `url`、`advertise_url` 或 `websocket_url`。这不等于删除其他服务或 HTTP/CDN/种子中无关的 url 字段；原生 Track 自身注册地址仍按自己的协议处理。
- 列表 1–8 项，每项最长 2048 字符；允许 quic/wss，ws 仅限 loopback 测试。拒绝凭据、fragment、重复项和非法端口；保留合法 WebSocket 路径及 query。共享 schema/Rust 校验使用相同 fixtures。
- 端点是无优先级集合，校验后固定排序存储/签发/比较，禁止静默丢弃重复或非法项；调用方偏好由 SDK 定义。列表更新、删除地址必须提高 identity_version；同版本不允许换地址或公钥。
- Track 请求携带支持的 scheme 列表，替代 punch_transport；Web 声明 wss（本地测试加 ws），原生按真实启用的连接器声明。Track 按交集及对应传输容量分配，返回完整签名身份端点集合；候选连接方再次筛选可连接入口。容量仍按 QUIC/WebSocket 两类统计，ws/wss 不算两份独立容量；查询不因候选的绑定传输类型不同而错误排除可互通信令的 Peer。
- **维护者已确认 TLS 方案（2026-10-29）**：QUIC 直连继续按 Track 下发的 Punch 公钥校验证书；WSS 在原生端与浏览器均使用公共 CA、域名、有效期校验。反向代理属于可信部署边界，终止公网 TLS 后通过本机或受控内网 WS 转发给 Punch，不向代理分发 Punch 身份私钥。首版不增加 Punch 应用层挑战签名证明，不关闭证书校验；既有票据、MAC、Peer 签名及 DTLS 指纹绑定保留。公共 CA 仅认证域名入口，不宣称直接认证后端 Punch 公钥。
- 原生优先 QUIC，WebSocket 使用明确端点，不再从 quic 地址换 scheme。明确 WSS 使用公共 CA/域名校验的代理入口，与 Web 一致；QUIC 继续校验节点公钥。调整原生现有 WSS 公钥 pin，不能拿代理证书冒充 Punch 节点证书。认证失败不降级绕过，非认证连接失败的回退受取消、总超时与资源预算约束。
- Peer 签名、票据、MAC、DTLS 指纹绑定和已提交块校验不变。公网 WSS/代理证书/TURN 验收仍单独记录，不由本地通过推定。

#### 执行顺序

- [x] E1 共享契约：更新 punch-discovery / punch-management、Rust PunchAssignment/PunchWire、签名凭据和跨语言 fixtures；确定规范排序与 scheme 能力请求。搜索所有实际消费者，覆盖 admin、platform、脚本、配置示例及测试桥。2026-11-07 已完成，消费者同步接线及本地验证见下方交付记录。
- [x] E2 Registry 与 SQL：直接修改 `apps/registry/migrations/0001_nodes.sql` 的建表语句保存 endpoints_json，增加 JSON 数组与服务类型约束；移除尚未上线的 0004_punch_websocket.sql。不新增 ALTER/双字段/回填迁移；已有其他业务建表文件不因本任务随意合并。同步注册签名校验、原子 upsert、防回滚、发现 RPC、管理返回及 CI 数据库前置检查。2026-11-07 收尾验收通过，见下方 E2 记录；未操作线上 schema。
- [x] E3 Workers Track：分配、候选、状态 schema、DO 身份账本和会话一致性全部接列表；直接更新 DO 建表 SQL，移除旧账本 ALTER 兼容分支。更新 scheme/容量筛选及测试，不再接受旧单地址协议。2026-11-07 新增 HTTP/持久状态场景及真实 Registry RPC 验收通过，见下方 E3 记录。
- [x] E4 Rust 服务与 SDK：更新原生 Track 静态节点配置/响应、Punch 注册凭据解析、配置与签发工具；同步原生 SDK 连接器、明确 URL 选择、WSS 公共 CA 校验和有界回退。删除推导 WSS URL 的逻辑，使用独立端口/路径测试防止退回猜地址。2026-11-07 收尾验收通过，补齐真实握手/取消/join 回退与原生配置/发现测试；公网证书成功路径仍另行验收。
- [x] E5 Web 与管理界面：更新 Wasm/JS 解析、Web SDK network/peer-dial 的 scheme 选择；节点详情正确列出多个端点，平台公开视图继续遵守现有字段脱敏边界。www 页面只消费 SDK，不直接选择 Punch 地址。2026-11-07 收尾验收通过；浏览器端点、候选过滤与管理消费者回归见下方 E5 记录。
- [x] E6 验收：共享 fixtures、Registry/Workers Track、Rust Track/Punch/core、Wasm、Web SDK 类型与 bundle、Chromium 控制面/原生互通/P2P 完整文件与刷新恢复回归；补列表重排、重复/非法项、无交集/容量满、同版本篡改、端点变更失效、WS 独立路径、取消/失败回退断言。原有 WebKit/native ICE 问题保持显式未通过，不掩盖为本轮回归成功。2026-11-07 当前工作区统一本地回归通过，跳过与公网边界见下方 E6 矩阵；不代表线上验收完成。
- [ ] E7 文档与上线清单：统一中英文协议、配置样例、SDK README、CI 与架构记录；在全新空库/空 DO 上初始化并验收。部署时协调暂停旧节点/客户端，初始化 Registry schema，清理 Track 旧会话、索引、缓存与身份账本后部署新控制面，重新签发 Punch 凭据、更新节点与两端 SDK，再验收公网发现/连接。失效的旧凭据不得继续部署使用。2026-11-07 文档/清单与全新本地状态初始化验收已完成；确切线上资源核实、切换/重签/公网验收未执行，总项保持未完成。

#### C7.E1 共享契约基础（2026-11-07）

以下保留 E1 首轮仅共享基础的历史记录。维护者随后明确要求持续推进直到 E1 完成，本轮已将 list-only 形状接入现有发现/管理 schema、Track wire/state、Registry claims 与 SDK 消费者，并联动处理原 E2–E5 中保证可编译/可运行所需的 SQL、状态和连接器接线。没有新增双协议兼容分支；E1 总项现已完成。E2–E5 的基础实现已包含在这次联动中，后续按各阶段剩余验收核对，不重复建设。C7.E 整体上线与完整验收不因此提前勾选。

- [x] `packages/punch-discovery/endpoints.ts` 与 `q2q_punch::endpoints` 提供同义的端点/能力集合校验、规范化、固定排序与重复拒绝。Rust 模块没有网络运行时依赖，可无默认 feature 编译到 Wasm；直接依赖工作区已有的 `url` 库，避免再写 host/IDNA 解析器。
- [x] TS `punchAssignmentSchema` / `punchIdentitySchema` 与 Rust 模块中的同名概念定义 list-only 形状，拒绝旧地址字段；保留节点 ID、标准 Base64 公钥、JS 安全整数身份版本边界。现有服务尚未消费这些新形状。
- [x] 约定 announce 新能力字段为 `punch_schemes`，取 `quic` / `wss` / `ws` 的非空无重复集合。Web 公网仅声明 `wss`，loopback 测试可加 `ws`；原生按连接器实际能力声明。节点绑定所用 scheme 不作为候选 Peer 能否互通的过滤条件。
- [x] 共用 `tests/fixtures/punch/endpoints.json` 与 `credential.json`。URL host/IDNA 与默认端口使用 WHATWG special authority 规则，QUIC 保留显式端口且不接受业务路径/query；WSS/本地 WS 保留路径/query。拒绝空 userinfo、空 fragment、零/空/溢出端口和控制字符。规范化前后各限制 2048 UTF-16 code units，结果最多 8 项；规范化后重复也报错，不静默合并。
- [x] 确认 `q2q-keygen` 保持通用；增加共享公知测试 seed 的签名对照与 claims 保留测试，不引入 Registry 专用 CLI 参数。JWT 必须先验证原始字节再比较规范身份；即使仅重排端点，修改 JWT body 后也不能沿用旧签名。
- [x] Registry/Track/WWW CI 加共享包类型及测试检查；端点/签名 fixture 变更触发三方应用门禁与 Rust CI。没有修改部署凭据或线上状态。
- [x] 将 list-only 形状接入现有发现/管理 schema、Track wire/state、Registry claims 和 SDK 消费者，并删除旧字段。2026-11-07 接线及回归已完成；下面首轮基础记录中的“尚未消费”说明不再代表当前代码。

实际消费者清单及接续点：

| 阶段 | 已核对的入口 | 必须一起处理的边界 |
| --- | --- | --- |
| E2 Registry | `apps/registry/src/auth.ts`、`nodes.ts`、`management.ts`、`index.ts`，`migrations/0001_nodes.sql`、未上线的 `0004_punch_websocket.sql`，认证/节点/管理/RPC 测试与 `.github/workflows/registry.yml` | Punch claims / D1 / 发现与管理返回统一列表；Track 自身的 HTTPS `advertise_url` 不误删；端点重排同身份，增删改必须提高版本；同步 SQL 与部署前置检查。 |
| E3 Workers Track | `apps/track/src/http.ts`、`state.ts`、`directory.ts`、`peer-state.ts`、`selection.ts`，discovery/state/selection/benchmark/runtime fixture | 分配按 scheme 交集和对应容量；持久身份账本改初始建表、去 ALTER；会话与候选一致性比较完整规范列表；历史 state/outbox 数据不得混读。 |
| E4 原生协议与状态 | `crates/q2q-track/src/state.rs`、`wire.rs`、`redis.rs` 及其 contract/selection/redis 测试 | 接新的共享端点类型；更新 Redis 序列化及 Track `v1.json` 相关样例，不给旧单地址 wire 留兼容读取路径。 |
| E4 原生服务与连接器 | `q2q-track-server/src/main.rs`、`api.rs`、`directory.rs`、`registry.rs` / `registry_tests.rs`；`q2q-punch-server/src/registry.rs`；`q2q-core/src/control.rs`、`punch_transport.rs` 及 query/punch_transport 测试 | 静态配置、Registry 发现/身份、announce 能力、URL 选择全部用列表；删除推导 WSS 地址与 WSS 公钥 pin，保留 QUIC pin；取消、有界回退与认证失败不降级。 |
| E5 Web / Wasm | `q2q-wasm/src/control.rs`、`scripts/check-control.mjs`；`packages/client-web/src/network.ts`、`peer-dial.ts`，network/p2p/native RTC fixture | 只选 Track 下发且当前 host 可用的 scheme；保留独立 WS 端口/路径测试，候选不按对方绑定传输误过滤；回归完整文件和刷新恢复。 |
| E5 管理与展示 | `packages/punch-management/index.ts`、`apps/admin/src/lib/server/registry-management.ts`、`routes/nodes/[id]/+page.svelte`；`apps/platform/src/lib/server/registry-nodes.ts`、`scripts/check-registry-nodes.mjs` | Admin 列出端点，平台公开节点视图仍脱敏；`apps/www` 只消费 SDK，不直接持有 Punch 地址逻辑。 |
| E6/E7 配置与文档 | `e2e/check-registry.mjs`、`deploy/track.toml`、原生测试帮助程序、SDK/Registry/管理包 README、中英文 Track/Punch/单节点部署文档 | 修改所有凭据样例及旧地址断言；公开配置不夹带密钥。上线前另列 Registry D1 与所有 Track DO 的精确重置范围，禁止自动删除其他业务资源。 |

首轮发现的预算问题：合法的 8 × 2048 端点列表会超过旧服务 token 的 8192 字节上限。现已将 Punch/Registry token 上限提高至 32 KiB、通用 keygen claims 输入上限提高至 32 KiB，并用最大 8 项 × 2048 的真实签名 JWT 覆盖 Registry 验证和 Punch 本地读取。Track 自身无端点集合的原生服务凭据仍沿用其独立预算。浏览器 Track JSON 响应上限调整为 256 KiB（Punch 帧仍为 64 KiB）。列表/page/快照仍分别受既有条数和总字节预算约束，不能据单节点极值推定最大规模性能。

本轮本地验证：共享 TS 79 项通过；Punch 38 项旧测试 + 7 项新契约测试、keygen 3 项通过；Registry 30 项、Workers Track 81 项回归通过；共享包、Registry/Track 类型检查与两个 Worker 的 dry-run 构建通过；主站 Svelte 检查（0 错误/0 警告）、Rust workspace check、Punch/keygen Clippy（warnings 为错误）、Punch 无默认 feature 的 wasm32 编译通过；CI 门禁 10 项通过（本机使用 Python 3.13，默认系统 Python 缺少 tomllib）。未重跑完整浏览器/P2P/大文件套件，未验证公网 WSS/TURN，未部署、提交或创建 PR。

#### E1 接线交付与最终验证（2026-11-07）

- 发现/管理共享 schema、Rust `PunchWire` 与 `PunchAssignment` 反序列化都复用同一端点校验；注册凭据先验证原 JWT，再规范身份比较。`AnnounceRequest` / Workers announce 要求 `punch_schemes`，拒绝旧 `punch_transport`；Punch 不再接受或返回旧单值地址字段，Track 自身 HTTPS 地址不误删。
- Registry 初始 SQL 保存 `endpoints_json`，区分 Punch 与 Track 的非空/空身份列，验证 JSON 数组和 1–8 项；原子 upsert 保留同版本身份约束。旧未上线 `0004_punch_websocket.sql` 移出 migrations，原文保留在本地临时备份，未对远端执行 SQL。CI 检查改为新列。
- Workers 身份账本直接建表保存完整规范列表，没有 ALTER/回填/旧字段读取；Peer 与 Discovery 元版本为 2，旧版本明确拒绝。Redis 原有仅节点 ID 的记录改为完整序列化身份，旧格式拒绝读取；临时独立 Redis 验证两端连接间完整身份、持有查询与原生 HTTP routes。
- 两种 Track 都按 scheme 交集及对应容量分配；WS/WSS 共用 WebSocket 容量。查询比较完整身份而不按对方绑定传输误过滤。Web 只选 host 可用的显式入口；Admin 节点详情列出全部端点，Platform 状态仍脱敏。
- 原生连接优先 QUIC，再 WSS / loopback WS，端点建连有 20 秒总预算，仅 transport 错误可继续；协议/认证/远端拒绝均停止。QUIC 保留公钥 pinning；WSS 明确使用 aws-lc provider 和现有 webpki-roots 公共 CA 根，不修改进程全局 provider，不使用自签节点证书替代公信入口证书。WSS 证书错误（含包装在 IO 中的 rustls 错误）归类认证失败，不触发降级。join 失败收敛并关闭资源，fallback 使用新票据。
- 公共 CA 根直接依赖复用已在锁文件中的 `webpki-roots`，避免 WebRTC/ring 与 QUIC/aws-lc 双 provider 导致隐式 TLS 初始化 panic；未切换 WebRTC 关键依赖。新增真实原生 WS-only 独立路径/query测试、自签但公钥匹配仍被 WSS 拒绝的反例、实际连接器注入的优先级/错误/总预算断言。
- 浏览器控制面/P2P 夹具使用不同于 QUIC 端口的 loopback WS 代理，URL 带独立路径/query；测试明确防止重新猜测地址。代理只转发信令，不代理 WebRTC 文件数据，结束时关闭自己创建的服务和临时资源。
- 配置、e2e 脚本、SDK/Registry/共享包说明及中英文 Punch/Track/安全/单节点文档已同步到新协议。主站仍只有 HTTP 产品界面，Web 发布未开始，本次不扩大这项产品能力。

本轮最终结果（不是历史数字）：

| 验证 | 结果 |
| --- | --- |
| `cargo test --locked --offline --workspace` | 502 通过、0 失败、21 ignored；opt-in/基准忽略不计通过。另行运行的 Redis/原生 SDK 验证见下行。 |
| 原生 Clippy / fmt | workspace all-targets warnings-as-errors 与格式检查通过。 |
| Redis 临时本地实例 | q2q-track redis_remote、原生 Track Redis HTTP routes 两项 opt-in 均通过；退出清理实例，不使用生产 Redis。 |
| 共享 TS / Registry / Workers Track | 79 / 34 / 80 项通过，相关类型检查通过。 |
| 浏览器控制面 | Chromium 148 / Playwright WebKit 26 合计 22 项通过，显式独立 WS 入口。 |
| 浏览器完整文件 P2P/刷新恢复 | Chromium 17 项通过；17 项 WebKit/native 按已知 ICE 限制跳过，不计成功。 |
| 原生 SDK 联调 | 真实原生 Track 及 Workers Track/Registry/Punch 控制面与无 CDN 8,388,731 字节 SHA-256 校验通过。 |
| HTTP / Vite-CSP / WWW 生产预览 | 64 / 2 / 12 项通过；Vite 使用实际 apps/www 的 8.3.5。默认根 Vite 解析到旧构建器的路径错误不计通过。 |
| Wasm | wasm32 编译通过；真实绑定与 Node 独立密码学/整数/数据帧/证明对照通过。 |
| 应用构建与文档 | Registry/Track dry-run、主站生产构建、Admin/Platform/WWW 类型检查（0 错误/0 警告）、中英文 docs build、CI 门禁 10 项与 diff 检查通过。 |

环境记录：本机缺 Playwright 1.57 所需的 Chromium 1200 二进制，使用已有 Chromium 148 的 `CHROME_PATH` 显式复测；没有关闭 sandbox/证书校验，也没有把首次环境失败算通过。WSS 本地反例测试发现并修复 TLS provider 初始化和证书错误分类后，全工作区重新通过。本轮未重复 1 GiB 大文件测量，未验证公网公共 CA 代理成功路径、真实跨 NAT、强制 TURN 或 WebKit/native ICE；远端 CI、上线状态重置和凭据重签未执行，未提交/推送/创建 PR。

下一步按 E2–E5 已落基础逐项核验剩余验收，再完成 E6 全矩阵和 E7 精确上线清单；不要重新实现已接线的共享契约或提前宣称 C7.E 全部交付。

#### E2 Registry 与 SQL 收尾验收（2026-11-07）

本轮核对并完成 E2，不重复修改已经接好的初始 SQL/签名契约，也不启动 E3 的新实现。

- 补充端点删除、新增、query 替换与公钥轮换的同版本冲突测试：拒绝时原节点行不变，提高身份版本后允许变更；节点 lease 过期也不能让仍有效的旧凭据复活旧端点。版本交错上报最终保留最高版本的完整端点及公钥。
- 覆盖 QUIC 主机大小写/前导端口零、WSS 默认端口、列表重排等同义身份，确认规范化后持久字段一致；非法/重复/旧 claims 在获取限流预算前被拒绝，上报正文不能夹带新旧身份字段。
- 最大 8 项 × 2048 端点的真实签名凭据完成 HTTP 注册、JSON 持久化、HTTP 发现与管理列表/详情一致性验证；Platform display 明确不泄漏 endpoints、原始 JSON 或公钥。
- workerd/Miniflare 真实 D1 + 两个独立 Service Binding entrypoint 同样测试最大端点集合，发现与管理均返回完整规范数组、没有旧字段。只读发现不能调用管理写入口的权限隔离保持通过。
- 发现并修复 CI schema 门禁仅验证列存在的缺口：新 `apps/registry/scripts/check-schema.ts` 只读查询 sqlite_master，比较当前初始迁移构建的四张受管表及其索引/触发器定义。不执行远端返回的 SQL；只忽略定义外空白/注释，不忽略字符串内容。旧 `advertise_url NOT NULL`、旧地址列、缺失 CHECK/索引、额外触发器及半迁移结构都阻止部署。此检查保守要求初始 schema 一致，不自动兼容手工结构变体。
- 门禁用当前初始 SQL、变异反例及 workerd 真正返回的 sqlite_master 结果验证，CI 部署步骤已接线；没有运行远端门禁、迁移或重建操作。Registry README 同步说明检查范围与上线前必须协调重置的边界。

本轮实际验证：Registry 类型检查、42 项测试、Worker dry-run 构建通过；Workers Track 的 80 项回归通过（含真实 Registry RPC）；`git diff --check` 通过。本轮未改 Rust 或传输逻辑，未重复整工作区 Rust/浏览器 P2P/大文件测试；不将 E1 的历史数字算作本轮结果。未提交、推送或部署，E3 为下一项。

#### E3 Workers Track 收尾验收（2026-11-07）

本轮以已实现代码为准补齐 HTTP/持久状态验收，没有重写生产分配、调度或认证逻辑。新增测试集中在 `apps/track/tests/endpoints.test.ts`、`discovery.test.ts` 和既有只用于测试的 `state-worker.ts` 适配器；测试 RPC 不进入生产入口。

- 通过真实 workerd HTTP announce 覆盖 scheme 交集、仅 QUIC 满/仅 WebSocket 满/两者均满、WS/WSS 共用容量、未知容量、停止接收、无交集及多节点筛选。符合能力的候选节点才参与分配；目录重排不会改变唯一可分配节点，响应保留完整端点身份，不只返回选中传输地址。
- 缺省/空/重复/未知能力集合、旧 `punch_transport` 字段均由 HTTP 返回 422；分配失败时不建立 Peer 会话。场景使用不同测试网络前缀隔离限流，避免将残留额度误判为功能失败。
- QUIC 宣告持有者、WSS 宣告查询者可以互相发现，候选不按对方绑定传输错误过滤。列表重排保留原会话；query 变更、端点删除、公钥轮换和节点移除使旧心跳返回 401。失败心跳不消耗 seq；恢复原身份的更高版本后，同一 seq 能正确续用。旧持有者的完整身份与当前目录不匹配时，实时文件索引仍在也不会被返回。
- 直接核对持久账本只有 id/version/endpoints_json/public_key，元版本为 2；多节点快照中先出现的有效身份升级，在后续节点同版本冲突时与快照一起回滚。非法/重复端点列表不污染账本。真正驱逐 Discovery DO 后重读，账本及快照一致保留。
- 经真实 Registry D1 / 只读 RPC 的额外验收覆盖等价 URL/列表重排的同版本更新、同版本删除拒绝、更高版本删除生效，以及 DO 驱逐后拒绝低版本复活旧端点。
- 验证旧 Peer/Discovery 元版本均明确拒绝读取/新写入，不静默 ALTER、回填或重建。修正原 TestPeer 旧版本注入方法误指 discovery_meta 的测试夹具错误，并实际执行此分支；生产版本检查不变。

本轮实际结果：Track 类型检查、86 项测试、Worker dry-run 构建通过；独立 `test:sdk` 通过（原生 Track 与 Workers Track 控制面、真实 Registry/Punch，以及无 CDN 8,388,731 字节文件完整 SHA-256 校验）。`git diff --check` 通过。测试均为新建临时本地实例，结束后释放；未执行线上 DO/Registry 重置、未提交/推送/部署。未重跑 WebKit/native、公网 WSS、跨 NAT、强制 TURN 或大文件容量测试，不扩展支持承诺。下一项为 E4 原生服务/SDK 的连接器与身份边界收尾验收。

#### E4 原生服务与 SDK 收尾验收（2026-11-07）

- 原生 Track 静态配置验证列表规范化、独立 WSS 端口/路径/query 保留，并拒绝旧单地址字段、混合新旧字段、空列表、重复、非 loopback WS、缺失 QUIC 端口和 fragment。原生 Registry 发现验证 QUIC/WS/WSS 能力及各自容量、完整身份返回、同版本等价重排、非法快照不污染缓存/身份历史、高版本删除端点。
- Punch 的环境变量和 token 文件两种读取路径均覆盖合法显式端点、非法/重复/旧单地址凭据及超预算文件，WSS-only / loopback WS-only 入口可进行本地配置检查。注册 JWT 的签名与权限仍由 Registry 验证，不把本地读取测试冒充服务端授权。
- 新增实际连接器优先级测试：先遍历 QUIC，再 WSS，最后 loopback WS，不将规范列表顺序误当传输优先级；原有 20 秒建连总预算测试继续通过。
- 新增真实本地 QUIC 握手反例：错误的 Track 公钥返回认证失败且不连接备用 WebSocket。保留真实 WSS 自签证书即使公钥匹配也拒绝的测试；没有关闭 CA/域名/有效期校验或增加生产信任覆盖开关。
- 新增真实 HTTP WebSocket 代理握手 401/403 反例，发现原先错误被归类 Transport、可能继续尝试其他端点。已修复为不含地址或授权细节的 Authentication 错误，测试确认备用端点不被访问；其余传输失败仍沿用既有有界回退。中英文 Punch 指南同步该边界。
- 新增实际挂起 WebSocket 握手取消测试，取消后宿主 TCP socket 关闭，不残留等待任务。新增 QUIC join 响应丢失的真实双传输回退测试：先接收并验证 QUIC join，再主动中断响应；SDK 通过显式 WS 入口用新票据完成 join，两张票据均通过实际签名验证，Peer 身份相同但 cid/nonce 不同，完成后收敛关闭连接。

本轮实际结果：`cargo test --locked --offline --workspace` 511 通过、0 失败、21 ignored；core 连接器测试现为 21 项，原生 Punch 50 项、原生 Track 36 项通过（另 1 项 Redis opt-in 默认忽略）。workspace Clippy all-targets warnings-as-errors、fmt 检查通过。真实临时原生/Workers Track、Registry、Punch 的 `test:sdk` 通过，8,388,731 字节无 CDN 文件 SHA-256 一致。中英文 docs 构建、`git diff --check` 通过。

本轮未运行公网 WSS 公共 CA 成功握手、跨 NAT/强制 TURN 或真实 WebKit/native 验收；未重复大文件容量/浏览器全矩阵和 Redis opt-in。未提交/推送/部署，不修改线上状态。E5 Web 与管理消费者为下一项。

#### E5 Web 与管理消费者收尾验收（2026-11-07）

本轮核对既有 Web SDK/管理接线并补测试，未修改生产传输逻辑或扩大主站功能。

- 在真实本地 Registry/Track/Punch 控制链上对 Track 返回做受控变异，浏览器验证：列表重排仍使用明确 WS 地址；仅 QUIC 分配报 PUNCH_UNAVAILABLE；旧 websocket_url、重复列表、非 loopback WS 和 fragment 均在打开 socket 前报 NETWORK_PROTOCOL_ERROR。announce 发送 punch_schemes，不发送旧 punch_transport，公开网络快照不包含地址/端点、会话密钥或 TURN credential。
- 查询响应变异覆盖完整列表候选保留精确端口/路径/query、仅 QUIC 候选被 Web host 跳过、混合旧字段的候选被拒绝。测试用 Wasm 生成真实身份公钥与 Peer ID，不拿不匹配的假 ID 绕过候选认证约束。
- 单独验证同时有 loopback WS 与 WSS 时优先显式 WSS，绝不从 QUIC 推导入口。该用例通过 Playwright WebSocket mock 只验证 host 选择与控制帧行为，不视为公网 CA 成功握手或真实 Punch 认证。真实本地独立 WS 端口/路径的控制面仍由其余用例验收。
- Admin readManagedPunch/readManagedPunches 消费规范列表并保留全部端点、不修改上游数组；旧单地址字段、空/重复列表和非法 WS 均拒绝。核对节点 Svelte 详情逐项遍历 endpoints、以文本元数据显示，不使用原始 HTML 或把端点变成任意可执行链接；本轮没有运行 Admin 可见浏览器视觉验收。
- Platform 节点公开列表 3 项回归确认只展示 Punch、白名单字段输出继续隐藏 endpoints/公钥/token。主站 WebSession 仍只消费 Client/Transfer，没有页面 Punch 地址选择器，P2P/发布 UI 不在本轮开放。
- 核对 Wasm 控制绑定只负责 Peer 身份/签名/协议消息，URL 由 JS host 与共享 schema 解析；没有为端点再建立第二套 Wasm/页面解析器。

本轮实际结果：Chromium 148 / Playwright WebKit 26 控制面 42 项通过；完整文件/纯 Peer 证明/刷新恢复 Chromium 17 项通过，WebKit/native 17 项按已知 ICE 限制跳过；Vite 8.3.5/CSP/Worker/Wasm bundle 2 项通过。Admin 54 项、Platform 节点脱敏 3 项通过；SDK、Admin、WWW 类型检查通过（Svelte 0 错误/0 警告），Admin/WWW 生产构建通过；Wasm 真实绑定与 Node 独立签名/整数/帧/证明检查通过；diff 检查通过。控制面首次重复运行遇到一次临时 Punch 启动退出（未确定原因），完整重跑 42/42 通过，不宣称此前启动失败已根治。

未提交/推送/部署、未操作线上数据；未验收公网 WSS 成功证书路径、跨 NAT、强制 TURN 或 WebKit/native ICE，也未重复大文件容量测量。下一项 E6 统一回归与剩余验收矩阵，不将这些已知缺口标为通过。

#### E6 统一本地回归与验收矩阵（2026-11-07）

本轮对 E1–E5 完成后的同一工作区重新运行整合回归，无生产代码变更。以下均为本轮结果，不累加历史阶段通过数字；不同套件可能重叠，不能直接求和当成独立场景数。

| 层次 | 本轮实际结果 |
| --- | --- |
| Rust 全工作区 | fmt、Clippy all-targets warnings-as-errors 通过；`cargo test --locked --offline --workspace` 511 通过、0 失败、21 ignored。 |
| Redis opt-in | 新建独立临时 Redis，q2q-track redis_remote 及原生 Track Redis HTTP routes 各 1 项通过，退出清理实例；不是生产 Redis。 |
| 共享端点 / Registry / Workers Track | 79 / 42 / 86 项通过，相关类型检查通过。Registry 包含真实 D1/Service Binding 及 schema 门禁反例；Track 包含能力分配、会话失效、持久账本/驱逐恢复。 |
| Admin / Platform / SDK Node 单测 | 54 / 3 / 11 项通过；Platform 数字仅指节点脱敏脚本，不是平台全量业务测试。 |
| Wasm | 生产 Wasm 构建及 wasm32 Clippy warnings-as-errors 通过；真实绑定对照 Node 独立签名、u64、身份 transcript、帧、Merkle 与稀疏见证检查通过。 |
| 浏览器合并套件 | Chromium 148 / Playwright WebKit 26 一次 runner 执行 storage/http/network/rtc/p2p/bundle：260 通过、0 失败、20 明确跳过；不包含 @large。 |
| 原生 SDK 联机 | 临时原生 Track 和 Workers Track/Registry/Punch 的控制面验收通过；无 CDN P2P 下载 8,388,731 字节，独立 SHA-256 一致。 |
| 应用检查与构建 | SDK 类型、Admin/Platform/WWW Svelte 检查（0 错误/0 警告）通过；Registry/Track Worker dry-run、Admin/WWW 生产构建通过，未部署。 |
| 主站实际页面 | 生产预览 Chromium/WebKit 12 项通过，HTTP 下载、保存与恢复等既有路径回归；主站未开放 P2P/发布。 |
| CI 影响判断 / 文档 | Python 3.13 的 10 项 app-impact 测试通过；中英文 docs build、diff 检查通过。远端 CI 未执行。 |

端点专项覆盖定位：

| 验收边界 | 主要测试入口 | 结果 |
| --- | --- | --- |
| 规范排序、列表重排、重复/非法 URL、签名篡改 | tests/fixtures/punch、Punch/Track endpoints.rs、共享 TS、Registry endpoints/auth/nodes | 通过 |
| 无交集、容量满、WS/WSS 共用容量、停止新分配、跨绑定传输候选 | Workers endpoints.test.ts / state-worker、原生 Registry tests | 通过 |
| 同版本增删改/公钥篡改、防回滚、旧字段拒绝、极值凭据 | Registry auth/nodes/schema、原生配置/Reporter/发现测试 | 通过 |
| 端点变更使旧会话/候选失效、持久身份账本原子回滚、DO 驱逐恢复、旧状态拒绝 | Workers endpoints/discovery/state、原生身份比较及 Redis 验收 | 通过 |
| 独立 WS 端口/路径/query、连接优先级、QUIC pin、WSS 自签拒绝、代理 401/403 禁止降级 | core punch_transport、浏览器 network | 通过；mock WSS 优先级不等于公信证书成功验证 |
| 取消/背压/超时与 20 秒建连总预算、join 丢响应后新票据回退 | core punch_transport、浏览器 network/rtc、HTTP/存储回归 | 通过 |
| Chromium/native 完整文件、Peer-only proof、真实 reload 重验与坏内容拒绝 | 浏览器 p2p/rtc、原生 test:sdk | 通过 |

明确跳过与仍未验收：

- 浏览器 20 项跳过中，17 项是 WebKit/native 完整文件 P2P，2 项是 WebKit/native RTC 双角色，均因已知 ICE 问题保留未通过；1 项是 WebKit 不具备的 Chromium CDP 清站点数据场景。它们不计为成功，不新增“WebKit 已兼容原生”的承诺。
- Rust 21 项 ignored 是 opt-in/基准/浏览器辅助项；本轮另外显式运行上述两项 Redis 和 test:sdk，不能把所有 ignored 算作已验收。
- 公网 WSS 公共 CA/代理实际成功链路、跨 NAT、强制 TURN、真实 Safari/Firefox/移动端尚未验收。公网失败回退不能用本地 WS 或 Playwright 路由 mock 替代证据。
- 本轮未重复 100 MiB/1 GiB 存储与系统保存交互、频繁提交放大成本、云端负载/成本测量；@large 从合并 runner 显式排除。既有 C5 历史记录不成为本轮性能结果。
- 本轮未进行 Admin 可见浏览器视觉验收，也没有全量平台计费/存储业务测试；本次 UI 相关范围为端点消费者、类型/构建和节点脱敏。

环境和执行边界：使用已安装 Chromium 的显式 CHROME_PATH、apps/www 的 Vite 8.3.5；本地服务和 Redis 均临时创建并收尾。未关闭 sandbox/TLS 校验，未运行线上迁移、资源重置、节点凭据签发或部署，未提交/推送/创建 PR。E6 标记的是约定范围内的本地统一回归完成，不表示 C7.E 或产品上线完成。下一项为 E7：明确部署/重置/回滚清单与新状态初始化验收；任何线上变更必须在精确范围确认后执行。

#### E7 文档、上线准备与新状态初始化（2026-11-07）

- [x] 中英文 Workers Track 正式部署指南加入端点列表切换/回滚章节，并明确历史线上冒烟不等于新协议已上线。保留既有隔离预发布说明，不另建重复部署手册。
- [x] 按实际仓库配置列出 Registry Worker/D1 标识、四个 Track DO binding/类、服务绑定/环境、Admin 管理入口、Platform HTTP、原生 Redis prefix 与 Punch/代理/SDK 的核对范围。标识来自配置，不冒充本轮已检查线上账号/namespace。
- [x] 列出完整状态：Registry node/rate/policy/audit/迁移账本，Track session/holding/outbox、file 成员/版本水位、discovery 身份/缓存/重试与限流桶。明确 policy/audit 需保留/重新应用，不能把节点默认可发现造成的策略丢失当成正常重置；不涉及 Platform 业务 D1、R2/S3、计费、账号或 TURN secret 的删除。
- [x] 推荐新 Registry D1 + 独立 Track Worker/DO namespace 的蓝绿方案，保留可恢复的整套旧栈；描述但不创建新资源。列出 main CI 自动部署协调、节点/客户端停写、空库 schema 门禁、绑定/TURN 初次配置、凭据重签和版本水位、探针、统一切流与续约期观察顺序，以及失败时禁止关闭认证/混用旧格式。
- [x] 回滚覆盖旧 Worker + 对应旧数据库/四类 namespace/Redis prefix、路由/管理绑定、旧协议节点与客户端；明确 revert 不恢复已删状态，D1 备份不包含 DO，过期会话必须重新宣告。停止/放量阈值及保留期由实际 SLO/操作窗口确认，不编造默认阈值。
- [x] 新增 `apps/track/tests/bootstrap.test.ts`：真实临时 D1 应用当前初始 SQL，检查 schema、初始空审计/Peer/Discovery，经 Registry Service Binding 冷启动加载新端点账本，再运行签名会话/跨绑定候选与失效测试；驱逐 Discovery 后账本保留，独立 namespace 仍为空。测试结束释放实例，不访问生产。
- [ ] 只读核实并填写线上账号、旧/新 Worker version、实际 DO namespace ID、D1/prefix/路由/管理绑定及备份映射，确认精确资源与执行授权。
- [ ] 公网 WSS 公共 CA 成功链路、跨 NAT/强制 TURN、云端目标规模与实际切换/回滚演练。
- [ ] 在确认范围内执行上线：新资源初始化、策略保留、凭据重签、节点/SDK 同步与路由/CI 协调。普通“继续”不替代删除/重建/发布所需的精确授权。

正式清单见 [中文](../docs/zh/guide/deploy-workers-track.md#端点列表协议切换清单) / [English](../docs/en/guide/deploy-workers-track.md#endpoint-list-cutover-checklist)。本轮实际验证：Track 类型检查、87 项（含新 bootstrap）通过；Registry 42 项回归通过；Track Worker dry-run 和中英文 docs 构建通过；独立原生/Workers Track + Registry/Punch `test:sdk` 通过，8,388,731 字节全文件 SHA-256 一致；diff 检查通过。未重跑整工作区 Rust/浏览器/大文件，不使用 E6 数字替代本轮结果。未提交/推送/部署，也未执行远端查询/资源创建/重置/凭据签发。

E7 的本地准备已交付，线上执行项保持未勾选，C7.E 不宣称全部完成。接续线上阶段前需明确目标资源及授权；若先继续 Web 发布开发，应由维护者明确调整“先完成 C7.E 再 C7.6c”的原顺序。

本次不执行删除或重建线上资源。维护者允许测试环境删库重建，不表示要自动删除其他业务 D1 或全账号资源；实施上线时应先列出确切 Registry D1 与 Track DO 范围。Track 含多个 DO/索引/持久 outbox，只删 Registry 节点行不能视为全量重置完成；采用受控 namespace 换代或精确清理，禁止遗留旧 wire 数据混用。


#### E7 线上资源只读核查（2026-11-08）

经授权核对 Registry/Track/Admin/Platform 的绑定和公开域名，确认 Registry 与平台业务库独立、
Track 使用四类 SQLite DO 且内部调用专用只读 RPC。未读取 Secret 值或修改资源。具体账号、数据库、
namespace ID、部署版本映射不放公开任务记录，执行前由操作方在受控环境再次核实。主站 Worker 当时未部署。

#### E7 Registry 生产 schema 只读核验（2026-11-08）

维护者在切换 Default 权限模式后明确要求重试，查询已成功。目标为 q2q-registry D1（[受控资源/版本标识省略]），仅查询 sqlite_master 的受管表定义和 d1_migrations 名称；未读取节点/策略/审计数据行或凭据。两条 SELECT 均报告 rows_written=0、changes=0、changed_db=false。

核实线上 nodes 仍为旧 schema：advertise_url TEXT NOT NULL，没有 endpoints_json，也没有 websocket_url。已应用迁移仅 0001_nodes.sql、0002_report_limits.sql、0003_punch_management.sql；管理策略/审计与限流表存在。相同迁移名称不表示当前源码修改后的初始 schema 已应用，不能再次 migrations apply 就期待旧 0001 被替换。

结论：当前线上库与新 endpoints 协议不兼容，不能直接部署新 Registry/依赖方；当前 schema 门禁应阻止该状态。继续保持旧生产栈，不修改表结构。下一步需维护者确认新 Registry D1 + 独立 Track Worker/四类 namespace 的蓝绿具体资源方案、成本及创建授权，并另行核实策略/审计保留与备份恢复映射；本次核查不授予创建、重置、切流或发布权限。


#### E7 MVP Registry 原地重建（2026-11-08，维护者调整方案）

维护者明确项目仍为 MVP，允许旧表删除重建，并逐项确认备份后重建 q2q-registry 的 nodes、report_limits、punch_policy、punch_operation、d1_migrations 五张表。此决定取代对这五张表必须蓝绿保留在线旧库的建议；不表示可删除 Platform/Admin 业务 D1、文件存储、Track DO 或自动部署服务。

已完成：导出旧 Registry 到本地受限目录，SQL 文件权限 0600；在临时内存 SQLite 成功恢复备份并验证五张旧表存在、旧 nodes 不含 endpoints_json。预览重建 SQL 后执行指定 Registry D1，15 条语句均 success：删除五表、按当前 0001–0003 建立新版表/索引、重建并登记迁移记录。Cloudflare 内部表未修改。备份路径仅保存在当前会话工具输出中，不将可包含审计记录的备份加入 Git 或公开发布。

重建成功返回不替代独立最终验收：后续只读 schema 门禁及空表 COUNT 查询被权限层拦截，尚未执行，所以尚未独立确认重建后的最终结构/行数。未部署新版 Registry；现有旧 Worker 与新版 nodes 不兼容，节点注册/发现可能暂时失败，不能宣称控制面已恢复。下一步须获准执行最终只读验证，并在明确部署授权后上线匹配版本 Registry/依赖方及处理 Track 旧状态。未提交/推送，未创建新资源，未改 Platform 数据库、Track DO 或存储内容。


#### E7 新 Registry 上线与 schema 复核（2026-11-08）

维护者明确授权生产 q2q-registry 的表行数/迁移名称只读核查、当前代码部署与公开健康/未认证接口冒烟。线上 schema 门禁部署前后均通过，确认重建结构与当前初始迁移一致。核查 nodes=0、punch_policy=0、punch_operation=0、report_limits=2；迁移记录为 0001–0003。限流行可能来自现有 reporter 重试，未读取节点/审计正文，不把它当作新凭据成功注册证据。

本轮 Registry 类型检查、42 项测试和 Worker dry-run 构建通过后，已部署 q2q-registry 新 version [受控资源/版本标识省略]，custom domain registry.q2q.app；保留既有 D1 binding/environment 与 Secret 配置，未读取 Secret 值。curl 公网验证 health=200（status ok）、未认证 discovery/nodes=401（invalid_token）。先前 Python urllib 健康请求返回 Cloudflare 1010/403，未将其误记为 Worker 通过，后用标准 curl 确认上述公开响应。

仅 Registry 已上线；未部署 Track/Admin/Platform，未重置 Track DO，未重新签发/分发 Punch 凭据、未更新原生节点。旧凭据不兼容，当前 nodes 为空，不宣称节点发现或端到端 P2P 已恢复。下一步需明确实际 Punch 节点及签发/部署权限，并确定四类 Track namespace 的测试状态重置方案；不影响 Platform/Admin 业务库与文件存储。原地 MVP 重建方案继续取代此前蓝绿建议，历史旧 Worker+旧库配对回滚需要本地备份恢复，单纯 revert 不能恢复旧 schema。


#### Punch v0.7.0 独立发版 PR（2026-11-08）

维护者要求先发布 Punch 镜像，再由维护者部署节点。已核实既有 Git tag / GHCR 镜像最高 v0.6.0，原生服务 manifest 历史仍为 0.1.0；本次将 q2q-punch-server 版本提高至 0.7.0。仅提交 endpoints 校验模块、Punch Reporter 凭据接线、必要 url 依赖/锁文件、测试 fixtures 和双语部署 README，共 10 文件，未将其他 Web/Track/平台未提交工作纳入发版。通用库的 Web feature 改造及 SDK/Workers 切换仍保留本地，发布提交仅增加 endpoints 模块，避免牵连未发布工作。

精确暂存快照验证：Rust workspace 496 通过、0 失败、19 ignored；Punch 库/端点/服务专项 95 通过；workspace all-targets Clippy warnings-as-errors、fmt 通过；实际二进制 --version 为 q2q-punch-server 0.7.0，release guard 8 项通过。验证快照是暂存内容而非含其他修改的工作目录。当前机器无 Docker，镜像构建与二进制版本检查由 main 发版工作流执行，不声称本地镜像已验证。

维护者明确授权公开推送/PR；已提交 67514884475e6e04bbd4eff5fb6d2d6b34db8c70，分支 claude/punch-v0.7.0，PR https://github.com/mivinci/Q2Q/pull/99。创建时 native release 规则检查成功，Rust 与 Track CI 待完成；后台等待 PR 检查。未合并、未创建 tag、未发布镜像。目标 ghcr.io/mivinci/q2q-punch-server:v0.7.0 需 PR 获准合并及 main Rust/镜像工作流通过后才可拉取；Track 包版本未提高，不应发 Track 镜像。部署节点仍由维护者执行，需新版 endpoints 凭据并保留原 Punch 身份。

Punch v0.7.0 PR #99 CI 后续核验（2026-11-08）：Rust（含三平台 HTTP、Windows CLI、Wasm）及 Track 联调等所有已执行检查成功，PR state=OPEN、mergeStateStatus=CLEAN。PR 阶段发布 job 只做门禁，Track 部署跳过；尚未合并或发布镜像，等待维护者明确合并授权。


Punch v0.7.0 发版完成（2026-11-08）：PR #99 已核实合并，main commit 4b66d9511ea1c76a2dc0521f9384d99806b47a5d；已同步本地 main，stash 恢复后逐文件 hash 核对其余 202 个本地修改/新文件完整保留。main native release run 42445238387 成功，Rust gate、镜像构建、实际二进制版本校验、Git tag 与 GHCR 推送均成功。punch/v0.7.0 已存在，ghcr.io/mivinci/q2q-punch-server:v0.7.0 与 latest 公共 manifest 均可读且 digest 相同：sha256:2b6c90eb3e8c10df2559b3b56da7d115cde4a846d97539cb11ce9622c584b7a9。镜像目标 linux/amd64；Track release job 版本未变，实际发布步骤跳过，未发 Track 镜像。节点部署由维护者执行，需改用新版 endpoints token、保留原身份并提高变更后的 identity_version；换镜像不代表旧 Track 状态或节点凭据已自动升级。


#### 香港 v0.7.0 凭据重签与追加信任（2026-11-08）

维护者明确先不处理成都，随后逐项授权本地生成新 Registry 签发密钥/香港 token、创建生产
REGISTRY_ADDITIONAL_JWKS（仅公钥）并部署匹配 Registry。香港 claims 保留既有节点 ID 与
Punch 公钥，endpoints 只有 quic://punch-hk-1.q2q.app:4433，identity_version=2，90 天有效期，
到期 2027-02-06T04:51:51Z。没有生成新的 Punch 身份。私钥和 token 存管理机受限目录，
权限 0600，不在仓库或对话中披露，也不上传节点之外的服务。

为保留原 REGISTRY_JWKS 且不读取其 Secret 值，增加可选 REGISTRY_ADDITIONAL_JWKS 读取，
总公钥数最多 16，重复 kid/私钥/非法附加配置 fail closed；缓存随两项配置变化刷新。
新增双信任与缓存轮换测试，Registry 44 项通过，类型检查/Worker dry-run/schema 门禁通过。
本机 workerd 平台 binary 缺失曾阻塞，npm rebuild workerd 后类型、真实 workerd RPC 测试和
构建重新通过。未修改依赖版本来规避问题。

部署 version [受控资源/版本标识省略]，REGISTRY_JWKS 原项保留，新增
REGISTRY_ADDITIONAL_JWKS；只确认名称/类型，不读取原 Secret 值。实际香港 token 的 GET
/v1/nodes 返回 403 forbidden，确认签名/claims 通过、Punch 不能越权访问 display scope，
没有使用该探针登记节点或写数据库。不是节点上报或端到端 P2P 验收。

等待维护者仅将香港新 token 安全替换进对应服务器 token_file 挂载，并以 v0.7.0 重建容器。
不要分发 Registry 签发私钥，不改香港已有身份文件；成都不处理。Registry 信任改动当前为
本地未提交代码直接部署，需在后续 PR 纳入，避免 main 自动部署旧认证代码使附加信任失效。
Track DO/版本及公网 WSS/跨 NAT/TURN 仍未完成；香港 QUIC-only 不表示浏览器 WSS 已可用。


香港新增 WSS 凭据（2026-11-08）：维护者提供 wss://punch-hk-1.q2q.app 并确认按当前实现使用根路径，随后明确授权重签及保存新凭据。复用已追加信任的现有 Registry 签发密钥，不生成新密钥，不改香港 Punch 身份。新 claims 规范 endpoints 为 quic://punch-hk-1.q2q.app:4433 与 wss://punch-hk-1.q2q.app/，identity_version=3，有效期 90 天，到期 2027-02-06T05:11:27Z。本地独立 Registry verifier 验签/字段与 token 0600 权限检查通过，旧 v2 文件保留，等待维护者替换服务器 token 并重启；未把 WSS 地址声明当作代理证书/公网连接已验收证据，未改变线上 Registry 配置或服务器。


香港部署反馈与公网入口核验（2026-11-08）：维护者报告 punch-hk-1 已部署。使用 Node 标准 WebSocket 连接 wss://punch-hk-1.q2q.app/，公共 TLS 证书校验启用，Upgrade 成功后立即关闭；未发送凭据/信令，不作为 Peer 身份或完整文件传输验收。维护者另行明确允许只读查询 Registry 的香港节点登记行，但 Cloudflare D1 API 返回 7403（账号无效或无权访问服务），查询失败，尚未核实其 version/identity_version/endpoints/租约；不将部署反馈或 WSS 握手等同于注册上报成功。未修改线上配置或 Track 状态。继续登记核查需恢复当前 Cloudflare 会话权限或由维护者提供脱敏上报日志。


Track MVP 状态切换（2026-11-08）：维护者授权同 Worker 四类新空 namespace 并部署。先上线 v3-endpoints，将 PEERS/FILES/DISCOVERY/RATE_LIMIT 全部绑定 *Endpoints 类，version [受控资源/版本标识省略]，保留 TURN 配置。随后维护者明确纠正 MVP 不保留旧测试状态，已按新要求移除旧类导出并追加 v4-drop-mvp-legacy deleted_classes：PeerState、FileIndex、DiscoveryState、RateLimiter；线上部署成功 version [受控资源/版本标识省略]。旧 namespace/对象数据删除不可恢复，不再保留旧栈回滚路径；仅保留新版活跃四类，未动 Registry、Punch、Platform 业务库或文件存储。配置/类型检查、87 项 Track 回归（换代首轮）、部署 dry-run 通过。公网 Track health=200；尚未完成新版线上签名控制面/端到端下载验收。当前生产变更仍在本地未提交代码中，main 自动部署旧配置可能不兼容，需后续纳入 PR。


#### 香港 + 新 Track 公网 SDK 完整文件验收（2026-11-08）

维护者明确授权向生产 track.q2q.app / 香港 Punch 创建临时 SDK 会话、持有索引和信令并传生成的约 8 MiB 内容。已运行公开 SDK opt-in 用例 public_sdk_downloads_p2p_and_verifies_full_file_hash：8,388,731 字节、无 CDN 内容/证明来源、真实 WebRTC，SHA-256 26c629edd55f2459e48fdfaf91b4a95733aadc15e95ceb299ec92441f753402a 一致；测试 1 通过（39.21 秒）。完成后 Publication.close、确认 Track 查询来源为空、关闭两端控制会话，临时文件由 tempfile 清理；残余 TTL/版本水位并非零残留承诺。

当前已部署新版 Registry（含附加公钥）、香港 Punch v0.7.0 由维护者部署并报告 reporting active、新 Track namespace 代际并删除旧四类状态。此结果证明公网发现/信令与本机双 SDK P2P 文件链路已可用。两 Peer 均在本机，未强制 WSS 信令、未强制 TURN，不证明跨 NAT 或浏览器完整下载；公网 WSS 根路径只另做过启用 TLS 校验的 Upgrade。成都保持未处理，不推测其可用性。相关 Registry/Track/SDK 更新仍大部分本地未合入 main，需尽快收敛 PR，避免后续 main 部署覆盖线上能力。


#### 浏览器公网下载与刷新恢复（2026-11-08）

维护者确认先推进浏览器公网下载，再进入 Web 发布；并明确授权线上临时会话/索引/信令
测试与最多三次隔离刷新诊断。新增 public-p2p.spec.mjs 显式 opt-in，通过环境变量提供
HTTPS Track；原生 publication 辅助器仅在独立 ALLOW_REMOTE 授权开关下接受远端，默认
仍本地。仅生成 786,449 字节测试内容，不用业务文件、不改线上配置、凭据或限流。

实际结果：Chromium 148 经生产 track.q2q.app 和 wss://punch-hk-1.q2q.app/ 完整下载、
暂停/继续均到达成功结束；全字节比对及独立 SHA-256 为
3ba31e034e8297dab9200e0e697a3440048b3d9d113d5460ad0b5cff9c324cd1。
纯 Peer 外置证明场景拆为独立发布后，先提交部分块、暂停、真实 page.reload、从本地见证
重验并继续完整下载，连续两次隔离运行通过；第二次新增恢复后独立 SHA-256 比对通过。
浏览器连接均为真实香港 WSS，无 WebSocket mock、无忽略证书，HTTP 内容请求为 0；
种子无 CDN 内容或 proof URL。页面位于本机 loopback 的测试 origin，不是 q2q.app 主站上线。
两端 Peer 均本机，不算跨 NAT 或强制 TURN。

保留失败边界：首次复用发布会话串联测试在后续 join/exit 触发 rate_limited；诊断仅记录
method/error code，不打印授权/会话/票据内容。修正本地 stress helper 的 200ms 心跳用于
公网时为 30 秒，场景之间按真实配额等待 65 秒，不放宽服务器限流。配额等待后的串联
长会话末段在纯 Peer 首块出现 CONTROL_TIMEOUT，未进入 reload，原因仍未定。拆分会话
两次成功不证明该长期复用超时已根治。一次隔离成功时也见 peer_offline 信令响应，但
有界候选重试后完成；没有把它改成静默无限重试。

测试辅助清理：异常时也先发送 close 命令让原生 publication 撤回，5 秒未退出再 SIGTERM，
7 秒兜底 SIGKILL；正常完成退出并清理本地临时目录，远端 TTL/水位不承诺立即零残留。
没有创建服务器持久内容或修改服务配置。新增公网场景默认 skip，不能由常规 CI 意外触发。

本轮验证：独立公网 reload 2 次各 1 通过；串联测试整体仍失败，只有先前完成下载/暂停阶段
可记为通过，不能写为整套公网测试 3/3。本地 p2p 17 通过、WebKit/native 17 已知跳过；
SDK 单元 11 通过、类型检查、core all-targets Clippy warnings-as-errors、diff 检查通过。
未提交/推送/部署。C7.6d 的公网 Chromium 下载子项有证据，WebKit/native ICE、跨 NAT、
TURN 与长会话稳定性仍保留未完成；下一开发项为 C7.6c Web 发布/部分块共享及主站接线。


#### C7.6c.1 Web SDK 完整文件发布（2026-11-08）

维护者要求接续浏览器公网下载里程碑后的本机发布。已实现 Client.publish 与 Publication
快照/ready/pause/resume/close，不依赖 Svelte 或主站账号，主站页面本轮不改。发布能力仅在
显式 Track 和 WebRTC host 可用时开放；首版一个 Client 最多一份完整文件发布，内联叶表，
256 KiB 块，最多 65,536 块。上传连接（默认 2）与读取预算（默认 1）各自配置、与下载额度
分离；每连接独立 source reader 和一个经过校验的块窗口。未实现部分下载共享、上传速率
限制、多文件同时发布或公网发布验收，C7.6c 总项不提前勾选。

Rust/Wasm 新增 UploadProofs（共享原生 Merkle ProofTree、BlockVerifier、Control proof）与
inspect_connect（票据签名/目标/audience/connection/hash 校验），JS 不复制 Merkle/票据编码。
入站 Punch 路由按已验内容分派，WebRTC 身份/DTLS 指纹沿用既有实现。服务 Bitfield、
Proof/Piece、RequestProof、PieceNack、Have/BlockDone、Cancel 完成握手；发送前重新验源
块，源变化不允许继续提供旧种子。Track 文件宣告/续约/撤回与 heartbeat/query 串行，完整
Punch 身份变化明确失败。每个 Client 注册的发布随 Client.close 先撤回/关闭再关网络，
准备中取消等待真实宿主 open/read/close 收敛，observer 重入关闭已验证幂等。

新增本地 publication.browser/spec 与 native_downloads_browser_publication opt-in：
Chromium ↔ 浏览器、WebKit ↔ 浏览器及 Chromium 发布 → 原生 SDK 完整文件校验通过，
文件 786,449 字节，SHA-256 为 3ba31e034e8297dab9200e0e697a3440048b3d9d113d5460ad0b5cff9c324cd1。
异常覆盖准备时关闭/暂停再准备、同 ready Promise、空文件、重入 close、源变化 fail/撤回、
来源可见性暂停/恢复/关闭、丢片 NACK 原协议修复。Wasm/Node 另对入站票据错目标/错 audience/
错连接/过期/篡改及 proof 的独立 SHA-256 叶哈希/稀疏见证重验做对照，均通过。

本轮验证：完整本地浏览器 storage/http/network/rtc/p2p/publish/bundle 合并 275 通过、
21 跳过（含新增 1 项 WebKit/native 发布未兼容，其他沿用既有已知限制）；之后分离上传/
下载预算的末次发布专项 15 通过、1 跳过，SDK 类型重新通过。Rust workspace 511 通过、
0 失败、22 ignored（新增原生接收浏览器辅助由 Playwright 显式执行）；workspace all-targets
Clippy、wasm32 Clippy warnings-as-errors、Wasm 实际绑定/独立签名/证明对照通过。SDK Node
11 通过、WWW 检查 0 错误/0 警告与生产构建通过。合并首轮有 2 项 WebKit 失败，根因是
发布协议夹具用隐私上下文 OPFS，改为有界内存目标隔离协议验证后上述合并回归通过；不将
此改动冒充 WebKit 系统文件保存验收。

CI WWW 增加 test:publish，公网 @public 测试仍独立 opt-in；本轮只用临时本地 Track/
Registry/Punch，无线上操作，未提交/推送/部署，未重复 1 GiB/大规模性能测试。首版接口与
限制见 client-web README。下一项 C7.6c.2 部分块共享，再推进主站接线；不以完整源发布
冒充已支持下载边下边传。


#### C7.6c.2 活跃下载的已提交块共享（2026-11-08）

已实现显式 Client.downloadShared / resumeDownloadShared，返回 transfer 与 publication；
普通下载默认不共享。首版共享仅绑定活跃存储写入会话，暂停/失败/取消/最终完成前先停止
上传并撤回再释放或 finish 目标，完成后不隐式后台做种；关闭分享句柄不停止下载。
恢复新共享句柄必须从原检查点/目标重新验证实际字节，旧共享不会因普通 resume 自动复活。
这明确区分两个句柄的生命周期，接口和调用例统一在 client-web README。

可信路径：新增 CheckpointSession.readCommittedBlock 在同一目标 IO 队列中检查当前
已提交位、读回实际字节与本地见证、重新验根，再返回独立 bytes + 原生 Control::Proof。
不重新打开 partial OPFS source，不绕过目标独占租约，不长期持存储锁等网络；dirty 位与
receivedBytes 不能进入上传位图。Transfer 的存储 commit+CAS 后回调产生 Have，初始
Bitfield 只取本次恢复后的提交记录；Track 完整性标记保持 false。缺块请求明确 Reject。

完整叶表复用 BlockVerifier 的共享 Rust ProofTree（同时消除 UploadProofs 的重复树）；
稀疏证明模式新增 PeerProofVerifier.proof_frame，先验证字节/见证再按原树路径恢复原生
Proof。JS 只调度存储与宿主 API，不维护另一套证明算法。Node/Wasm 独立检查重建 proof
可再次验证、损坏 witness 明确拒绝；未知/损坏内容不能以位图已登记绕过验证。

本地 partial 场景：字节已 flush、metadata CAS 阻塞期间空 Bitfield/请求拒绝；CAS 成功后
同一连接收到 Have 并得到逐字节/证明验证通过的块；缺失下一块继续拒绝。覆盖 Chromium
真实 OPFS 在活跃写入会话中的有界读、CAS 失败不发布、pause/complete 先撤回、独立关闭
publication 后 transfer 继续、同一检查点重新打开共享只宣告重验块。外部篡改已提交内容的
反例发现只关闭连接却没撤回发布，已修复为所有 partial readback 错误触发共享失败/撤回，
复测通过。稀疏证明场景使用真实原生 origin、浏览器中间节点和浏览器验证端，无 HTTP 证明
来源，已提交块及持久见证成功再转发；不称作完整三节点文件级调度验收。

本轮最终本地浏览器合并套件（排除 @large/@public）289 通过、0 失败、23 明确跳过；
新增 partial 14 项通过、2 项跳过（WebKit/native sparse 与 WebKit 私有上下文 OPFS），
原有跳过沿用。稀疏场景曾一次出现 PEER_AUTH_FAILED，隔离复测和最终合并均通过，未证明
该偶发认证失败根因已解决。Rust workspace 511 通过、0 失败、22 ignored；workspace 与
wasm32 Clippy warnings-as-errors、SDK 类型、Wasm 实际绑定/独立证明、SDK Node 11 项、
WWW 类型（0 错误/0 警告）/构建、CI gate 10 项和 diff 检查通过。未运行公网或改变服务配置，
未重复大文件容量；未提交/推送/部署。

能力边界：此项是可信部分块上传及动态可用性，不扩展现有 Web 下载器为完整原生调度器。
当前候选穷尽退回 HTTP/失败，不无限等待唯一部分来源将来的块；完成后长期继续共享需显式
完整源 publish。多份同时发布、速率限制、原生多 Peer 竞争、跨 NAT/TURN/公网部分转发仍未
验收。主站未接分享/P2P，C7.6c 总项继续未完成，下一项 C7.6c.3 页面接线与实际 UI 验收。


#### C7.6c.3 主站消费 SDK（2026-11-08）

apps/www 已按确认设计接本机发布、种子保存、P2P/HTTP 下载、真实任务/控制会话快照与完成缓存显式
继续分享，页面无哈希/Peer/块状态机。分享任务只在前台存活，File/原种子/私钥不持久化，停止/卸载
调用 SDK 收敛；缓存被发布占用时禁止清理，关闭分享后才可删除。部分块 API 不默认启用，下载后分享
也默认关闭。PUBLIC_Q2Q_TRACK_URL 为公开静态构建参数，框架 3 使用 src/env.ts 的声明，不沿用旧
$env/dynamic/public。HTTP-only 空值保留，官方配置仅为地址、不含凭据。细节和结果见 web-client W5
最新记录及中英文用户指南。

本轮最后生产预览 22 项通过、2 项已知 WebKit/native 跳过；WWW 类型 0 错误/0 警告、生产构建、
diff 检查通过。先前开发模式20/2通过；最终新增确认取消/中英文路径在生产预览验通。新增本地 UI P2P
双向原生用例实际 hash 一致，缓存来源接力在原发布停止后仍完成下载；没有运行线上或部署新主站。
C7.6c 的 SDK 与页面本地接线子项已完成，但完整 W5/W6/W8 与 C7.6d 公网/跨网络验收仍有缺口，
C7 总项不因页面已接就提前完成。下一项优先跨设备与错误/恢复矩阵，再决定主站上线。


Web 主站本机异常/恢复收尾（2026-11-08）：维护者选择暂跳过跨设备、跨 NAT、强制 TURN。
完成本地来源离线重试、真实 Punch 中断/重启后重新选源发布、Chromium Peer-only seed 的
提交/暂停/实际刷新/重选种子重验/导出哈希、准备失败不导出种子且移除释放任务、活跃分享
刷新不恢复假在线。失败分享移除不可用的停止菜单，不引入假成功或自动无限重连。最终 WWW
生产预览 31 通过、3 已知 WebKit/native 跳过；SDK network/publish/partial/p2p 88 通过、
20 跳过，类型/构建/10 项 CI gate/diff 通过。详见 web-client 最新记录。未提交、未部署，
未执行线上测试；跨设备等暂跳过仍不是已完成，移动/大文件/公网 UI 也未因此验收。


Web 主站大文件完整链路验收（2026-11-08）：临时本地真实服务/两个 Chromium 磁盘 profile，
实际页面发布 File、导出种子、P2P 下载 100 MiB 与 1 GiB，提交后取消再检查点重试，最终
第二 OPFS 文件流式导出并由 Node 有界分块 SHA-256 独立比对，两项通过。双浏览器进程树
RSS 采样峰值约1.91/1.93 GiB，含共享页重复计数，不能说独占内存上限。原生 picker 缺失
时 >32 MiB Blob 路径正确拒绝，不误报 OPFS 导出为系统文件保存。详细实测数字/边界见
web-client 大文件记录与 apps/www README。本轮未扩大到真实系统保存/移动/WebKit/公网
大文件，跨 NAT/TURN 仍暂跳过。普通 UI 31 通过/7 明确跳过，WWW 类型及 diff 通过；未部署。


Web客户端本机最终收尾（2026-11-08）：维护者要求一口气验完可执行项。实际生产预览58通过/8明确
跳过，含24组浏览器×宽度×语言×主题的真实任务矩阵，216张截图已看代表性图并对照确认稿，修复
320px英文主按钮换行与单任务英文计数。设置持久主题/跟随系统及真实浏览器配额失败/缓存清理通过。
SDK本地整合289通过/23跳过，Rust511通过/22ignored，Wasm/SDK单元/类型/Clippy/fmt/10项gate通过。
100MiB/1GiB页面传输在本轮前段独立验通（OPFS非系统文件目标）；无线上操作、提交或部署。
细节见web-client最终验收记录，后续不重复重新安排已完成本机项；跨设备/NAT/TURN暂跳过、真实系统
保存/移动后台/物理磁盘故障/全部无障碍合规和公网长会话稳定性均不冒充通过，C7总体不能据此全部完成。


PR #100 首轮 CI 修复（2026-11-09）：Rust check/wasm 被 CI Rust1.99 的新增 chunks_exact_to_as_chunks
lint 阻止，本机1.96未提示。Wasm 32字节叶表和33字节见证改为as_chunks数组读取，长度校验与证明
语义不变；本地workspace/wasm32严格Clippy、Wasm真实绑定与独立proof/signature检查通过。
WWW 在浏览器安装前失败，ubuntu-latest 变为26.04、Playwright1.57不支持，固定该job为24.04；
没有禁用WebKit测试或降低警告门槛。其他首轮Registry/Track/Admin/Platform/Docs检查已成功；
修复推送后远端结果待核实，不将本地验证当CI成功。


PR #100 WWW 第二轮 CI 修复（2026-11-09）：远端运行 42501340539 的 SDK 存储套件
59 通过、56 失败、1 跳过，失败均在 Ubuntu WebKit。核对 Playwright 1.57.0 固定的
WebKit 上游 revision 486de399887bc8fa8a69e2f194ebc9476589a08a，StorageAPIEnabled 与
FileSystemEnabled 默认只在 Apple 平台启用；不是业务下载校验失败，也不能以 macOS
本地通过声称 Linux WebKit 支持。WWW workflow 改为 Chromium/ubuntu-24.04 与
WebKit/macos-15 双平台矩阵，各浏览器命令显式传 --project，报告按浏览器分别上传，
不增加 skip。影响判断独立 job；部署独立 job 等待整个检查矩阵成功，只允许 main 的
push/workflow_dispatch，PR 不接触凭据。部署 job 在自身 runner 重建，不跨平台复用构建目录。

本轮本地验证：WebKit storage 57 通过、1 沿用 CDP 专属跳过；Python 3.13 app-impact
10 项通过；Ruby YAML 解析及浏览器矩阵/参数/部署依赖断言、diff 检查通过。默认 python3
为旧版、缺少 tomllib，改用已有 Python 3.13 后通过；actionlint 的外部 go run 被权限阻止，
未执行。未重跑完整浏览器/UI 矩阵；未提交、推送或部署，远端新矩阵尚未执行。


PR #100 后续 CI 排查（2026-11-09）：25d3835 远端运行 42505382281 的 macOS
WebKit storage 57 通过、1 既有跳过，HTTP/bundle/network/RTC/partial 等已执行套件通过或
沿用已知 skip；发布 lifecycle closing 断言失败。该测试以固定 20ms 猜测 Wasm 初始化后
source.open 已进入，在较慢 runner 上关闭得过早。关闭与准备中暂停用例均改为等待源的
显式 entered 信号再触发操作，不改产品生命周期语义。本地两用例重复 12 轮共 24 项通过，
完整 WebKit 发布回归 7 通过、1 既有 native ICE 跳过，SDK 类型检查、11 项单元、10 项
app-impact 及 diff 检查通过。

同轮 Chromium storage 为 57 通过、1 失败：open cancellation 清理时 removeEntry 报
NoModificationAllowedError。fixture 现保留用例原始异常，避免 finally 清理异常覆盖根因；
未添加重试、等待或 skip，不宣称文件占用问题已修复。本地 Chromium 重复运行尚未进入
用例（缺少对应浏览器或指定系统 Chrome 路径不存在），安装对应版本尚在进行。新提交的
远端 CI 结果待核实；本记录不是完整浏览器/UI 回归通过声明。
