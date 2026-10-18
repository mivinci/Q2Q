<div align="center">

<img src="docs/public/logo.svg" width="180" alt="Q2Q">

# Q2Q

<p>
  <a href="https://github.com/mivinci/Q2Q/actions/workflows/docs.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/mivinci/Q2Q/docs.yml?branch=main&label=CI" alt="CI">
  </a>
  <a href="https://github.com/mivinci/Q2Q/stargazers">
    <img src="https://img.shields.io/github/stars/mivinci/Q2Q?style=social" alt="GitHub Stars">
  </a>
</p>

<p>
  <a href="https://docs.q2q.app/">Docs</a> ·
  <a href="https://platform.q2q.app/">Platform</a> ·
  <a href="docs/spec.md">Design</a> ·
  <a href="https://github.com/mivinci/Q2Q/issues">Issues</a> ·
  <a href="LICENSE">License</a>
</p>

<p>
An open-source peer-to-peer framework for distributing large files and delivering
video on demand or live streams, with verified transfers and CDN fallback.
</p>

</div>

## What is Q2Q?

Q2Q lets peers exchange file data directly when possible and fall back to an HTTP
CDN or origin when peer-to-peer delivery is unavailable or slower. Each downloaded
block is verified against a Merkle proof, so peers and delivery servers do not
need to be trusted with the content.

The project includes a web platform for managing files, seeds, projects and
distribution settings, alongside Rust libraries and a command-line client for
building and transferring Q2Q seeds.

## Components

- **Q2Q Platform** — manage files and seed publications, configure storage and
  Track networks, and organize work in personal or team projects.
- **Q2Q Core** — network-independent Rust primitives for identity, seed files,
  Merkle proofs and local storage. The same core can be compiled to WebAssembly.
- **Peers and CLI** — publish and download files over the Q2Q data plane, using
  peer-to-peer transfer with HTTP range requests as a fallback.
- **Track and Punch** — self-hostable control-plane services. Track helps peers
  discover one another; Punch relays connection signaling. They help establish
  connections but do not carry file data.

## Get started

### Use the platform

Open [platform.q2q.app](https://platform.q2q.app/) to manage files and seeds in
the hosted workspace. See the [documentation](https://docs.q2q.app/) for setup
and usage guides.

### Use the command-line client

Install the CLI from a checkout of this repository:

```sh
cargo install --path crates/q2q-cli --locked
```

Publish a file and create its seed, then give the seed to a downloader:

```sh
q2q --track https://track.example.net publish ./model.bin --seed ./model.q2q
q2q --data-dir ./receiver-state download ./model.q2q --output ./received.bin
```

The publisher must remain online while serving peers. Use separate state
directories for simultaneous clients. See the [CLI guide](https://docs.q2q.app/guide/cli)
for configuration, transfer progress and resume behavior.

## Develop

The platform and documentation site use Node.js 24 (see `.node-version`). From
the repository root:

```sh
npm ci

npm run platform:dev    # platform
npm run docs:dev        # documentation site
```

Useful checks and builds:

```sh
npm run platform:check
npm run platform:build
npm run seed-processing:check
npm run seed-processing:build
npm run docs:build
```

Rust crates can be built with Cargo; install the WebAssembly target when working
on the browser package:

```sh
rustup target add wasm32-unknown-unknown
cargo check --workspace
```

## License

Q2Q is released under the [MIT License](LICENSE).
