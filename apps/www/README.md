# Q2Q Web

`q2q.app` 的浏览器客户端，采用与 platform 相同的 SvelteKit 3、TypeScript、Tailwind 4、shadcn-svelte/Lucide 与 Cloudflare adapter。已接 `@q2q/client-web` 的本机完整文件分享、P2P/HTTP 下载、种子导出、磁力链粘贴下载与分享侧复制磁力链，不包含平台账号 Auth 或云端上传。分享/P2P 页面已部署；跨设备矩阵尚未完成，Safari 双端互通正在排查。后文标注日期的本地验收与未部署状态属于当时记录。

## 本地运行

```sh
npm ci
npm run wasm:build
npm run www:dev
```

默认只监听 `127.0.0.1:5188`，端口被占用会退出，不自动另开 5174，也不会停止其他应用。修改源码会热更新。需要明确指定其他端口时，使用 `npm run dev --workspace @q2q/www -- --port 端口`；不同时启动两个不知用途的 dev 服务。

```sh
npm run www:check
npm run www:build
npm run www:test
WWW_PREVIEW=1 npm run www:test
```

最后一项要求先 build，测试生产预览而非开发服务器。Playwright 使用独立临时磁盘 profile、本地 Range/CORS 来源与测试端口 5180，结束后关闭测试服务。可用 CHROME_PATH 指定 Chromium；默认 Chromium/WebKit，需安装匹配 @playwright/test 的浏览器。Wasm 构建需要仓库相同的 Rust target 与 wasm-bindgen CLI。报告在 `.tmp/www-playwright`，截图和 trace 不进 Git。

Web CI 按自身及工作区依赖影响运行类型、构建、SDK 单元测试、Chromium 打包冒烟与 Chromium/WebKit 生产预览页面测试。SDK 的存储、HTTP、信令、RTC、P2P、部分共享、发布和磁力链完整契约已拆到独立的 `Web SDK` 工作流，页面或 UI 源码改动不触发这套完整回归；PR 只执行检查，不注入部署凭据；相关更改合入 main 后，所有检查通过才部署 `q2q-www` Worker，并通过 Custom Domain 绑定 `q2q.app`。也支持在 main 手动触发（不填 base_sha 时完整执行）。复用仓库 Secret `CLOUDFLARE_API_TOKEN` 与 Variable `CLOUDFLARE_ACCOUNT_ID`，与 platform 一致；Token 还需允许管理 q2q.app 所属 Zone 的自定义域名。主站没有 D1 或服务端 Secret，无需数据库迁移。配置已接线，尚未实际触发线上部署。

## 已实现路径

- 选择/拖入 `.q2q` → SDK 本地解析 → 文件名/大小预览 → 明确开始 HTTP 下载。
- “下载”对话框粘贴 Q2Q 磁力链 → 本机解析 → 通过 Track/Punch 从在线 Peer 取得并校验元数据 → 同一预览与下载流程；
  只支持 `urn:sha256`，页面不使用链接中的 `tr` 或 `dn`。经验证的最小种子缓存在本机 IndexedDB（不存链接），刷新后
  “重新粘贴链接”优先用缓存。分享行菜单可“复制磁力链”，剪贴板被拒时给出可手动复制的只读链接。
- SDK 处理 Fetch、范围响应、Rust/Wasm 完整性校验、检查点和关闭；页面只订阅快照与发送命令。保留 SDK 种子句柄原对象
  （`SvelteMap`），不交给 Svelte 深代理。
- 显示收到/校验/提交进度；暂停/继续、取消、新任务重试、详情与显式删除暂存。刷新后仅将历史提交量当作提示，重新选择同一内容身份的种子后由 SDK 重验字节再恢复。
- 完成内容暂存于 OPFS。支持 showSaveFilePicker 的浏览器在用户点击时立即打开原生保存窗口，然后流式导出；只有 user-file 收据才显示已保存。取消窗口或导出保留缓存。
- 无原生保存接口时，仅为不超过 32 MiB 的文件构建有界 Blob 并交给下载管理器，显示“已提交浏览器保存”，不确认系统最终落盘。大文件明确提示限制，不分配整文件 Blob；可以在支持原生保存的浏览器重新下载。
- 复用共享品牌、控件和浅/深/系统主题；中英文偏好独立于 platform，以 host-only Cookie 保持 SSR 与页面一致。主站没有登录/头像，也没有跨域平台会话。

## 本地状态与边界

任务元数据按任务键保存在当前 origin 的 localStorage；检查点在 SDK IndexedDB，内容在 SDK OPFS。每个标签页只写自己的任务更新，新增任务不重写整个历史列表；SDK 租约保护字节和活动检查点。跨页看到的其他任务只是持久提示，不能推断另一个页面的实时进度。主动传输中清缓存可能受原生文件锁阻挡；删除失败时保留可重试入口。

只保存文件显示名、大小、内容 ID、缓存 ID、检查点 ID、历史提交量、完成提示与来源类型（种子/磁力链），不保存种子正文、磁力链原文、磁力链的 `dn`/`tr`、来源 URL、授权、私钥或原生文件句柄。刷新后恢复要求重新提供原种子或重新粘贴原磁力链，更新来源授权的种子也必须匹配同一内容身份。任务历史是可损坏的 UI 提示，不能绕过 SDK 内容/目标绑定与字节校验；异常记录不会阻断新下载。SDK 的磁力链元数据缓存按内容哈希存最小种子字节，读取时重验哈希，损坏记录视为未命中。

浏览器本地空间、授权、来源 CORS/Range 与网络限制仍适用；Peer-only 种子使用显式 Track 尝试 P2P。此阶段不配置 SourceProvider 与平台 grant，不自动刷新平台授权；过期来源需要重新领取有效种子。只有前台页面任务，不承诺手机后台或关闭页面后常驻。

## 设计和验证

[design](design/README.md) 保存用户确认的目标视觉，含尚未实现的本机分享入口。真实页面已接“分享文件/打开种子”两条路径，不把原型文件或进度混进用户任务。

本地 2026-10-28：Chromium 148 / Playwright WebKit 26 的正常磁盘 profile，真实页面通过 HTTP 下载、导出后独立 SHA-256 比对、暂停与页面刷新恢复、错误恢复种子、非法种子/损坏内容/过期授权、取消/重试、多标签页记录隔离、删除缓存和语言/主题操作。Node/浏览器测试使用生成的约 2 MiB 内容，不表示新的大文件 HTTP 容量、真实 Safari/iOS/Firefox、公网来源或 P2P 已验收。SDK 的大文件存储和原生文件保存验收记录见 packages/client-web/README.md。


## 分享/P2P 接线（2026-11-08）

用户流程、支持边界与隐私说明见正式 [中文指南](../../docs/zh/guide/web-client.md) / [English](../../docs/en/guide/web-client.md)。
PUBLIC_Q2Q_TRACK_URL 是 src/env.ts 声明的公开静态构建变量，默认官方 Track，空值为 HTTP-only；
可在构建/开发命令环境中改为受控测试 Track。不要在该变量放密钥或带授权 URL，改值需要重新构建。

分享任务只持有当前页面 SDK Publication、种子句柄及只读快照，不存 File/种子/私钥到历史；
关闭/刷新需重选源文件。下载任务历史/OPFS 保持既有检查点恢复。UI 展示准备、等待接收、发送/连接、
暂停/停止/错误；不把分享发送量换成下载进度。完成缓存再分享为显式菜单操作，默认关闭，分享期间
缓存删除禁用。独立分享 Client 与下载 Client 都由 WebSession 持有并在页面卸载时关闭。

本地最终生产预览 22 项通过、2 项 WebKit/native ICE 跳过：包含此前 12 项 HTTP/恢复、页面分享→
导出种子→另一页面 P2P 下载→导出独立 SHA-256、窄屏生命周期、确认取消/详情不中断/中英文、
完成缓存显式再分享（关闭原发布后仍可下载）、缓存删除保护、原生双向互通。WebKit 使用正常磁盘 profile。
测试将官方或测试域名的 Track 请求转到临时本地真实服务，不发送线上流量；生成约 768 KiB / 2 MiB
内容，不等于新大文件容量验收。开发模式先前 20 项通过/2 跳过，最终新增语言/取消用例在生产预览验收。
原生系统保存窗口沿用 SDK 独立验收，本轮页面导出使用有界浏览器下载管理器路径。

分享/P2P UI 已实现不等于上线：未部署 q2q-www、未验收跨设备/跨 NAT/强制 TURN、WebKit/native、
真实移动后台或公网平台授权刷新。SDK 侧部分块共享 API 没有自动打开，页面也没有后台常驻承诺。


### 本机恢复补充验收

2026-11-08 生产预览最终 31 通过、3 WebKit/native 已知跳过。补充覆盖无来源失败后原来源恢复
重试、临时 Punch 真实停止/重启后重新选文件发布、Peer-only 下载提交块后刷新重选种子恢复、
准备失败禁用保存种子/移除释放发布额度，以及活跃分享刷新不伪造在线历史。测试不使用生产服务。
SDK 相关本地回归 88 通过、20 已知跳过。跨设备/跨 NAT/强制 TURN 经维护者决定暂跳过，
不是验收通过；主站仍未部署。移动后台、真实权限撤销/磁盘耗尽和大文件容量不在本轮范围。


## 大文件实际页面传输（opt-in）

`tests/large-transfer.spec.mjs` 只有 TEST_LARGE_TRANSFER=1 时运行，默认普通测试明确跳过。
需要至少 5 GiB 临时可用空间，Chromium 磁盘 profile、--workers 1，并先构建生产预览。
测试文件仅生成到本轮临时目录，以 1 MiB 块写入并计算 Node SHA-256；真实页面 File 选择、
SDK 发布/种子导出、另一独立 profile 的 P2P/OPFS 下载、提交后取消/检查点重试和大文件导出。
所有 Track 请求转到临时本地服务，不访问生产。

```sh
npm run www:build
TEST_LARGE_TRANSFER=1 WWW_PREVIEW=1 npm run www:test -- --grep @large-transfer --project chromium --workers 1
```

需要 CHROME_PATH 时显式指定本机浏览器；代码不会关闭浏览器 sandbox 或证书验证。
报告写入 .tmp/www-large-transfer-100.json / www-large-transfer-1024.json，并附加到 Playwright。
500ms ps 采样对两个独立浏览器进程树 RSS 求和，含共享页重复计数且可漏短峰，不是独占内存上限。

2026-11-08 Chromium 148 / macOS 两组本轮实测：

| 文件 | 准备 | 取消后重试的剩余下载 | 流式导出 | 独立 SHA-256 读回 | 双浏览器 RSS 采样峰值 |
| --- | --- | --- | --- | --- | --- |
| 100 MiB | 0.87s | 5.87s | 0.39s | 2.31s | 1.91 GiB |
| 1 GiB | 3.88s | 60.00s | 2.90s | 23.65s | 1.93 GiB |

这不是公网吞吐：两端、Track/Punch 与内容数据面均本机，下载计时从取消后重试起算，不能用它当全程带宽。
取消时分别已有 10 MiB / 9.5 MiB 已提交块，重试后最终文件 SHA-256 与源文件独立流式计算一致。
100 MiB 为 e28ecf908cea4e9855dd0873b859c19ca827301e41498d7c672dd7b033cdce24；
1 GiB 为 7af06c63b15a40ee3dffd5fb2c1fe11bfb8ba64ae50f29eb4b08bae5ea3b1196。

下载后 origin 用量约 100.58 MiB / 1028.10 MiB，额外导出后约 200.58 MiB / 2052.10 MiB；
删除应用缓存和导出文件后约 595 KiB / 4.10 MiB（含浏览器数据库保留分配/元数据，非零用量承诺），
最后关闭浏览器并删除本轮 profile 与生成源文件，释放测试资源。

无原生保存 API 的 >32 MiB Blob 路径正确拒绝，不创建任意大整文件 Blob。本轮注入 picker
返回第二个 OPFS 文件句柄，验证实际应用流式导出和有界 1 MiB 读回后的 Node SHA-256；
UI 未误报系统文件保存成功。这是 OPFS 流式导出验收，**不是本轮真实系统保存窗口、用户文件
最终落盘或断电持久化验收**。WebKit、Safari.app、Firefox、手机、真实配额/磁盘耗尽、跨 NAT/
TURN 与大文件公网路径不在本轮结果中；任意大小能力不因这两组通过而成立。


## 最终本机 UI 验收

2026-11-08 最终生产预览普通套件58通过、0失败、8跳过。新增ui-matrix实际驱动Chromium/WebKit、
320/390/1280宽度、中英文、浅深色共24组及系统主题持久偏好，覆盖分享/确认/暂停/详情/设置/
下载暂停/待保存/错误，并检查无横向溢出、模态边距、键盘/焦点、不可信长文件名及未捕获JS错误。
216张实际截图在 .tmp/www-playwright/results/ui-matrix-*；查看代表性截图，对照已确认design修正
320px英文按钮并排及单任务英文计数。WebKit/macOS全控件键盘导航使用Option-Tab。

CDP真实1MiB配额测试验证空间提示与失败缓存清理。8项跳过含3WebKit/native、4大文件显式opt-in、
1WebKit不具备CDP配额。此前独立100MiB/1GiB实际传输结果仍按对应边界引用，不算这次普通套件通过项。
SDK整合289通过/23跳过，Rust511通过/22ignored；类型/构建/文档与CI gate通过。
这不是全部浏览器、物理磁盘故障、WCAG/屏幕阅读器或真实系统保存窗口验收。跨设备/跨NAT/强制TURN
暂跳过，主站仍未部署。不要将本机viewport当成真实手机后台或公网UI证据。


## 失败诊断（开发版本）

失败任务的详情展示 SDK 原始错误码及可选脱敏连接阶段/状态/候选计数。无 SDP、IP、URL、
票据、Peer 身份或 TURN 凭据；不自动上传、不持久化到任务历史。此改动尚待部署，
不表示 Safari 跨设备问题已经修复。线上排障说明见中英文 Web 客户端文档。
