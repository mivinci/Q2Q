---
layout: home

hero:
  name: Q2Q
  text: 一个开源 P2P 网络框架
  tagline: 可用于大文件分发、音视频点播与直播，在提升传输效率的同时节省 CDN 带宽
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
    details: 密钥即身份。没有注册流程，也不需要一个服务端来背书你是谁
  - icon: 🛡️
    title: 端到端校验
    details: Merkle 证明锚定在签名过的 info.root，恶意 peer 与恶意 punch 都改不动内容
  - icon: 🔀
    title: 两种来源
    details: 每个 block 独立决定走 CDN 还是 P2P，冷启动全走 CDN 也能下完
---
