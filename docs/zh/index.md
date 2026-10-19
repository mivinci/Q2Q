---
layout: home

hero:
  name: Q2Q
  text: 一个开源 P2P 网络框架
  tagline: 面向大文件分发与视频点播，支持端到端校验与 CDN 回退
  image:
    src: /logo.svg
    alt: Q2Q
    width: 320
    height: 320
  actions:
    - theme: brand
      text: 指南
      link: /zh/guide/what-is-q2q
    - theme: alt
      text: 开放平台
      link: https://platform.q2q.app

features:
  - icon: 🔑
    title: 自签身份
    details: 本地生成密钥对，公钥就是 Peer 身份；无需注册，也不依赖中心身份服务
  - icon: 🤝
    title: 先授权，再连接
    details: 目标 Peer 先决定是否接受，获准后才交换 SDP 与 ICE 信息；拒绝可在连接建立前结束请求
  - icon: 🛡️
    title: 端到端校验
    details: 每个 block 都按种子中的 Merkle 根校验；数据无论来自 Peer 还是 CDN，都不必信任来源
  - icon: 🔀
    title: P2P 与 CDN 按块协同
    details: 每个 block 可独立选择 P2P 或 HTTP 来源；Peer 不可用或速度较慢时，CDN 会兜底
  - icon: ↻
    title: 下载中断后可续传
    details: 已校验的数据块和证明会保存在本地；中断后继续下载，无需重下已完成的块
  - icon: 🏠
    title: 控制面可自行部署
    details: Track 和 Punch 可自行部署，负责节点发现与连接信令，不承载文件数据
---
