//! Q2Q 核心库：网络无关的身份、种子、Merkle 证明与本地存储。
//!
//! 本 crate 不发起任何网络连接——所有 I/O 限于本地磁盘（store 模块）。
//! 网络相关的部分（传输、消息编解码、调度）在别的 crate 里，调用本 crate。
//!
//! 模块按实现顺序填充，见 todos/proposal-q2q-core.md：
//! - T2 `canonical`：确定性编码，info_hash 与票据签名的地基
//! - T3 `merkle`：Merkle 树、证明与校验
//! - T4 `seed`：info 编解码、info_hash、磁力链
//! - T5 `identity`：Ed25519 身份、peer_id、自签票据
//! - T6 `store`：block 四态、piece 位图、落盘
