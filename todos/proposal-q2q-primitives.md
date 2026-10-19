# Proposal: `crates/q2q-primitives`（阶段 1）

状态：草案。确认后按 T1 → T7 逐项执行，每项单独验收。

## 1. 背景

`docs/spec.md` 定义了协议，实现上需要先有一块**网络无关、可单测**的地基。阶段 1 的四个模块（canonical / merkle / seed / identity）加一个本地存储（store）都不发起网络连接，适合放进同一个 crate 先做出来，用 CLI 就能验证整条链路。

## 2. 决策

**语言用 Rust。** 这块逻辑对确定性和内存安全要求高（哈希、签名、状态机），Rust 的类型系统能把 block 四态表达成编译期检查的状态，避免"忘了校验就标记完成"这类错误。

**单 crate + 内部 module，不拆多 crate。** 四个模块都依赖 canonical 编码；拆成 `q2q-identity` / `q2q-merkle` 等会让 canonical 的每次改动跨 crate 传播，而 canonical 恰恰是最不能随便动的东西。将来某个模块真有独立复用价值（比如只想要 Merkle 校验）再拆出去，那时接口也稳定了。

**与现有结构共存。** 仓库根同时有 npm workspace（`docs`，VitePress 站点）和 cargo workspace（`crates/`），两者互不干扰，也不互相参与构建。

## 3. 任务拆解

| # | 任务 | 内容 | 验收 |
| --- | --- | --- | --- |
| T1 | 脚手架 | 根 `Cargo.toml` 作 workspace、`crates/q2q-primitives` 骨架、`.gitignore` 加 `target/` | `cargo test` 通过（0 个测试也行） |
| T2 | canonical 编码 | 确定性序列化规则与实现 | 向量测试：同一输入永远同一字节；键序、Unicode、整数编码 |
| T3 | merkle | 建树、单 block 证明、区间批量证明、校验 | n=1/2/3/5/7、末尾短 block、篡改（数据/叶子/内部节点）均正确 |
| T4 | seed | info 编解码、`info_hash`、磁力链、5.2 校验规则 | 往返一致；`v≠2`、`block_size` 非 2 的幂、`leaf_hashes` 长度不符均拒绝 |
| T5 | identity | Ed25519 密钥、`peer_id` 派生、票据签发与校验 | `peer_id` 固定向量；过期 / `aud` 不符 / `pub` 与 `iss` 不符 / 签名篡改均拒绝 |
| T6 | store | block 四态、piece 位图、in-place 落盘 | 状态转换、partial 恢复、重启后重校验 |

**顺序理由**：T2 是地基（T3/T4/T5 都依赖它）；T3 先于 T4，因为 `info.root` 是 Merkle 树根，seed 只是把它记进结构体；T6 依赖 T3（校验）和 T4（`block_size`）。

命令行工具（原 T7）不在这个 crate 里做：核心库只提供 API，CLI 是它的消费者之一，放到独立的 `q2q` crate。它同时也承担"无网络跑通整条链路"的验收角色。

## 4. 依赖

| 用途 | crate |
| --- | --- |
| SHA-256 | `sha2` |
| Ed25519 | `ed25519-dalek` |
| base32 + bech32 校验和 | `bech32`（校验和部分）或 `data-encoding` + 自实现 |
| info 序列化 | `serde` + `serde_json`（配合 T2 的 canonical 规则） |
| 错误类型 | `thiserror` |
| 属性测试 | `proptest`（dev-dependency） |

依赖从 T2 开始逐个加，不在 T1 一次性全塞进去——每加一个就在 commit 里说明为什么需要它。

## 5. 测试策略

- **向量测试**（必须有）：`peer_id` 派生、`info_hash`、Merkle 树根都要有固定输入输出的常量，跨实现比对靠它们
- **属性测试**：merkle 适合 proptest——随机长度的文件、随机 block_size，断言"建树 → 证明 → 校验"恒成立
- **篡改测试**：每个校验函数都要有对应的失败用例，只测成功路径等于没测

## 6. 开放问题

1. **canonical 用 JCS（RFC 8785）还是自定义二进制？** 倾向 JCS——info 本身是 JSON 结构，用 JSON 的确定性子集不必另发明格式。但 JCS 的字符串转义规则要仔细实现，这是最容易出错的地方
2. **store 落盘用什么？** 简单 `File` + `write_at` 就够（in-place 写入），不必上 mmap；等真测出性能问题再说
3. **是否支持 no_std？** 建议不支持——store 需要文件系统，`std` 省事得多，过早追求 no_std 会拖慢阶段 1
