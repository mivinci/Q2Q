---
layout: home

hero:
  name: Q2Q
  text: An open-source P2P network framework
  tagline: For large-file distribution and video on demand, with end-to-end verification and CDN fallback
  image:
    src: /logo.svg
    alt: Q2Q
    width: 320
    height: 320
  actions:
    - theme: brand
      text: Guide
      link: /guide/what-is-q2q
    - theme: alt
      text: Platform
      link: https://platform.q2q.app

features:
  - icon: 🔑
    title: Self-signed identity
    details: Generate a keypair locally. Your public key is your identity — no registration or central identity provider required
  - icon: 🤝
    title: Consent-first connections
    details: The target peer accepts before SDP or ICE details are exchanged, so unwanted requests stop before setup
  - icon: 🛡️
    title: End-to-end verification
    details: Every block is checked against a Merkle proof rooted in the seed, whether it came from a peer or a CDN
  - icon: 🔀
    title: P2P and CDN, block by block
    details: Choose P2P or HTTP for each block; use CDN when peers are unavailable or slower
  - icon: ↻
    title: Resume interrupted downloads
    details: Verified blocks and proofs persist locally, so an interrupted download can continue without fetching them again
  - icon: 🏠
    title: Self-hostable control plane
    details: Run Track and Punch yourself. They handle peer discovery and signaling without carrying file data
---
