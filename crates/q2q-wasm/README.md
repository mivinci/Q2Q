# q2q-wasm

q2q-primitives 的薄 Wasm 绑定，供 JS/TypeScript 增量生成内容元数据。计算在 primitives 中；本 crate 不读取文件、不联网，也不管理平台任务。

## 构建与验证

在仓库根目录执行，需要 Rust 和项目要求的 Node.js：

```sh
rustup target add wasm32-unknown-unknown
# 工具版本应与 Cargo.lock 中的 wasm-bindgen 一致。
cargo install wasm-bindgen-cli --version 0.2.129 --locked
npm run wasm:build
npm run wasm:check
# 可选：8 GiB 合成字节流基准，不创建相同大小的磁盘文件。
npm run wasm:check -- 8192
```

本 crate 同时是工作区 npm 包 `@q2q/wasm`。绑定、TypeScript 类型和 Wasm 产物输出到被 Git 忽略的 `pkg/`，由 package exports 暴露；业务代码通过包名导入，不访问 Cargo 的 target 目录。使用 `--target web`，宿主自行初始化预编译模块，不依赖浏览器 DOM。

```ts
const builder = new SeedBuilder(size);
try {
  for await (const chunk of source) builder.update(chunk);
} catch (error) {
  builder.free();
  throw error;
}
// finish 消耗 builder，即使失败也不能再使用它。
const result = builder.finish(name);
try {
  const seed = result.seed_bytes();
  const proof = result.proof_bytes();
  const infoHash = result.info_hash();
  // 由宿主保存产物；这些 Uint8Array 是独立副本。
} finally {
  result.free();
}
```

长度使用 JS 安全非负整数，块大小沿用协议默认值。空文件遵循 core 的空树语义；平台当前的非空文件限制由平台输入校验执行。

`proof_bytes()` 是完整的原始叶子哈希表，每块 32 字节；不是每块单独生成一份路径。`finish()` 的 Seed 不内嵌叶子表，需要后续装配证明地址；`finish_inline()` 将叶子表内嵌，供平台第一版鉴权下载使用。两者都不填来源 URL，也不自动发布或做种。后续由 core 的 Seed 模型填入持久的 `proof_list`、Track 与 CDN 地址，来源列表不改变 info_hash。

core 默认启用 `storage` 和 `random`，保留现有 SDK API；此适配禁用默认 feature，排除磁盘存储模块及系统随机数生成入口。

内存随块数增长而非文件字节数增长；Merkle 构建、Wasm 输入输出副本也占内存。脚本报告的是本地耗时及 Wasm 线性内存高水位，不包含全部 JS 内存，更不等同于 Cloudflare 线上 CPU 计费或限额验证。平台模块打包、队列消费及 S3 流式读取已接线，详见 apps/platform/README.md；线上尚未部署验证。
