# Q2Q

<p align="center">
  <img src="docs/public/logo.svg" width="110" alt="Q2Q">
</p>

一个开源 P2P 网络框架。

想分享文件的人打开 app 或调 SDK，本地做种并上报——**不需要自建任何服务端**。

框架本身提供 Track（发现）、Punch（信令）、STUN / TURN（穿透）这些组件，
由运营方部署；内容发布者这一方不需要提供任何东西。

完整设计见 [`docs/spec.md`](docs/spec.md)，构建后在线阅读见 GitHub Pages。

## 它跟别的 P2P 有什么不一样

| | Q2Q | 通常的做法 |
| --- | --- | --- |
| 身份 | 密钥即身份，peer 自签票据，没有注册流程 | 服务端签发 JWT，需要先注册 |
| 授权 | 开放：`info_hash` 即 capability | 依赖应用方后端判定谁能看什么 |
| 完整性 | Merkle 根锚在签名过的 `info` 里，端到端自证 | 信任来源服务端或种子站点 |
| 回源 | 种子里带 CDN 地址，由发布者自己维护 | 平台提供存储与桶凭证 |
| 连接 | B 显式同意后才建立，拒绝时不触发 ICE | 直接交换 SDP |

组件：Track（发现）、Punch（信令）、STUN / TURN（穿透）。它们由运营方提供，
但**信任被压到最低**——即使完全恶意，也改不了内容，只能窃听或拒绝服务。

## 目录

```
docs/       设计文档（VitePress 站点 + spec.md）
e2e/        构建产物的端到端自检
packages/   预留，暂无内容
```

## 开发

```sh
npm install

npm run dev      # 文档开发服务器
npm run build    # 构建文档站点
npm test         # 构建 + 端到端自检（图渲染、404、控制台报错）
```

自检用 playwright 驱动系统 Chrome，不额外下载浏览器。

> 本机若遇到 esbuild 临时目录被安全策略拦住，用 `TMPDIR=.tmp npm run build` 绕开。
