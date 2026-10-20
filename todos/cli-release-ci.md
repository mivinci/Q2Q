# CLI 发版和 apps CI 影响判定

## 已确认方案

先隔离无关 apps 构建部署，再自动创建版本 PR，最后审批正式发布。
单人维护：mivinci 可自行审批，关闭 self-review 限制及管理员绕过。
正式说明见中英文 [CLI 发版](../docs/zh/guide/cli-release.md)。

## 实现

- [x] apps 固定检查名、轻量 gate、锁定 WASM 完整依赖闭包比较，未知保守运行；main 部署同步过滤。
- [x] Prepare CLI release 输入稳定版本，只更新 CLI manifest/lock，创建版本分支及 PR；不自动合并。
- [x] 显式 dispatch Rust 与 apps gate（带基线），处理 GITHUB_TOKEN 创建 PR 不触发 CI 的限制。
- [x] 手动发布和标签入口共用审批与四平台构建流程；锁定 commit，验证 main 归属、版本及该提交最新成功 Rust CI。
- [x] 审批前确认 Environment 有维护者 reviewer、允许自审且禁用管理员绕过。缺失时停止。
- [x] 审批后创建标签并直接调用 reusable workflow，避免 GITHUB_TOKEN 标签事件不触发的问题。
- [x] 构建二进制版本检查；附件上传到草稿再发布，草稿可重试，公开产物不覆盖、标签不移动。
- [x] 中英文文档。
- [ ] GitHub cli-release Environment 配置：当前 API 返回 403 Resource not accessible by integration；需管理员在页面设置。
- [ ] 确认仓库允许 Actions 创建 PR（当前 token 无权查询该设置）。
- [ ] 合并后实际运行版本准备和发布审批。当前不发版，不创建测试版本标签。

## 验证

Python AST 语法检查、全部工作流 actionlint（v1.7.12）、git diff --check 通过。本轮未新增或运行测试，也不实际创建版本 PR/标签或触发发布；PR CI 结果待追加。
