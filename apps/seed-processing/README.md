# 种子处理 Worker

独立的 Cloudflare Queue 消费者，负责种子与证明生成、发布产物复制和定时回收。平台通过共享契约发消息，本应用通过共享 S3 适配器访问对象；不依赖 SvelteKit。

从仓库根目录运行：

```sh
npm ci
npm run seed-processing:build
npm run seed-processing:check
npm run platform:check:processing
```

构建需要 Rust Wasm target 与匹配版本的 wasm-bindgen CLI，见 `crates/q2q-wasm/README.md`。build 只做 Wrangler dry run；最后一条运行现有的 SQLite、模拟 S3 与真实 Wasm 集成回归。

本地 secrets 使用此目录的 `.dev.vars`，模板见 `.dev.vars.example`。与平台联调使用根脚本 `platform:dev:workers`，共用平台目录下的本地 D1 状态。数据库迁移继续由平台管理。

CI 配置在 `.github/workflows/seed-processing.yml`，只部署本消费者。共享的 `packages/s3-storage`、`packages/seed-processing-contract` 和 Wasm/core 变更会同时触发平台工作流。

Wrangler 名称保留为现有的 `q2q-platform-processing`，复用已部署的 Worker、secrets 和队列消费者配置。修改目录名不会创建新的生产资源。
