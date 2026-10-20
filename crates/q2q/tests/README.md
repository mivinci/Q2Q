# CLI 测试

```sh
cargo test -p q2q
```

普通测试通过独立可执行进程覆盖参数、环境变量、文件保护及离线恢复；
`shutdown.rs` 通过源码的 cfg(test)/path 入口覆盖私有初始化生命周期。
每个进程使用独立临时目录，不读取开发者的 Q2Q 环境变量或身份文件。

真实服务回归需先运行一组新的本地 Track/Punch，避免此前会话和按 IP 限流干扰：

```sh
Q2Q_TEST_TRACK_URL=http://127.0.0.1:8080 \
  cargo test -p q2q --test cli local:: -- --ignored --nocapture
```

该用例仅在 Unix 编译，启动真正的发布及下载 CLI 进程，验证中断续传、字节一致、
SIGINT/SIGTERM 退出码、撤销宣告及完整离线恢复；种子含 CDN 且首选 Track
不可达时仍能切换到种子 Track，中断后不加 `--resume` 自动恢复。进程由 guard 清理；
Track/Punch 由启动它们的操作者负责关闭。临时文件在用例结束时删除。
