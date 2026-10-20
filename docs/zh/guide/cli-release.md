# CLI 发版与 CI

## 一次性仓库设置

在仓库 **Settings → Environments** 创建 `cli-release`：

- Required reviewers 添加维护者 `mivinci`。
- 关闭 Prevent self-review，允许单人维护时自己审批。
- 关闭 Allow administrators to bypass configured protection rules。
- 若限制 Deployment branches and tags，允许 main 分支及 `v*` 标签。

发布工作流会读取并校验审批配置；未配置、禁止自己审批或允许管理员绕过时停止，
不会自动建立一个无保护环境继续发布。当前连接可能没有 Environment 管理权限，
上述设置需仓库管理员在 GitHub 页面完成。

在 **Settings → Actions → General → Workflow permissions** 允许 GitHub Actions
创建 pull requests。工作流显式申请 contents、pull-requests 和 actions 写权限；
不需要保存个人访问令牌。组织或仓库策略仍可能限制这些权限。

## 准备版本

在 Actions 手动运行 **Prepare CLI release**，选择 main，填写稳定版本 `X.Y.Z`，
不带 `v`（例如 `0.3.0`）。

工作流从最新 main 创建 `release/cli-vX.Y.Z`，只更新 CLI 的 Cargo.toml 和 Cargo.lock
包版本，创建版本 PR；版本必须递增且标签不能已存在，不覆盖已有分支。
默认 GITHUB_TOKEN 创建 PR 不会触发 pull_request 工作流，因此随后显式 dispatch
**Rust** 到该版本分支，并触发两个 apps 轻量 gate，保留必需检查名称。审查 PR 和 CI 后手动合并，准备流程不会自动合并或发布。
若创建 PR 的权限未开启，分支可能已经推送：启用权限后从该分支手动开 PR，
并手动运行 Rust，勿强制覆盖分支。

## 审批并发布

1. 版本 PR 合并后，等待 main 对应提交的 Rust CI 成功。
2. 在 main 上手动运行 **CLI release**。
3. 工作流确认版本与锁文件一致、提交属于 main、该提交最新 Rust CI 成功，
   并检查 Environment 审批配置。
4. 在该次运行中查看版本和 commit SHA，点击 Review deployments，自己批准 `cli-release`。
5. 审批后创建不可移动的 `vX.Y.Z` 标签，直接调用共享四平台构建工作流；
   不依赖 GITHUB_TOKEN 推标签触发另一个工作流。
6. 每个平台运行二进制 `--version` 核对版本；全部成功后生成 SHA256SUMS，
   创建/更新草稿并上传完整附件，最后公开 Release。

仍支持手动推送匹配版本的 `v*` 标签。此入口也校验 main 归属、CI 和审批，
不能跳过 Environment。标签入口中标签已由操作者创建，审批发生在构建发布之前。
选定目标后 main 的后续变化不会改变本次构建提交。

构建或草稿上传失败时重跑失败任务；也可重新从同一提交的标签运行。
重试只允许相同标签/提交，草稿附件可以补传；已公开 Release 不覆盖，标签不移动。
若手动发布提示 main 当前提交缺少 Rust CI（例如仅文档更新），先在该 main 提交手动运行 Rust。

## apps 影响判定

Platform 与 Seed Processing 保留固定检查名称，在 PR/main 运行轻量 gate：

- 应用自身、实际共享包、primitives/wasm、根 Cargo.toml 变化时运行对应构建。
- 只有 Cargo.lock 涉及 Rust 依赖时，比较前后 q2q-wasm 的完整锁定依赖闭包，
  包括版本、来源、校验和和传递依赖；独立 CLI 版本变更不会导致 apps 重建部署。
- 无法读取基线或解析依赖时保守执行；手动 dispatch 未提供比较基线时始终执行；版本准备自动传入基线，仅执行影响判定。
- 无影响时跳过安装、检查、构建和部署，但固定检查成功结束，避免必需检查永久等待。
- 根 npm 配置及锁文件仍保守触发；尚未按 npm 依赖闭包精确过滤。

PR 不获取部署凭据；部署仅在有影响的 main push/手动运行执行。
因为保留固定检查，GitHub 页面仍会显示 apps 工作流，但无影响时只有轻量判定。
