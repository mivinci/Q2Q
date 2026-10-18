<div align="center">

<img src="docs/public/logo.svg" width="220" alt="Q2Q">

# Q2Q

<p>
An open-source P2P network framework for large-file distribution and
audio/video streaming (VOD and live) — faster transfers, less CDN bandwidth.
</p>

<p>
  <a href="https://github.com/mivinci/Q2Q/actions/workflows/docs.yml">
    <img src="https://img.shields.io/github/actions/workflow/status/mivinci/Q2Q/docs.yml?branch=main&label=CI" alt="CI">
  </a>
  <a href="https://github.com/mivinci/Q2Q/stargazers">
    <img src="https://img.shields.io/github/stars/mivinci/Q2Q?style=social" alt="GitHub Stars">
  </a>
</p>

<p>
  <a href="https://docs.q2q.app/">Website</a> ·
  <a href="docs/spec.md">Design</a> ·
  <a href="https://github.com/mivinci/Q2Q/issues">Issues</a> ·
  <a href="LICENSE">License</a>
</p>

</div>

Publishers seed from their own machine and announce to the network — no backend
of their own required. The framework ships Track (discovery), Punch (signaling)
and STUN/TURN (hole punching); those are operated by whoever runs the network,
while the trust placed in them is kept minimal: a fully malicious Track or Punch
cannot alter content, only eavesdrop or refuse service.

## Command-line client

```sh
cargo install --path crates/q2q-cli --locked
q2q --track https://track.example.net publish ./model.bin --seed ./model.q2q
q2q --data-dir ./receiver-state download ./model.q2q --output ./received.bin
```

Keep the publisher running and send the seed to the recipient. Use separate state
directories for simultaneous clients. See the [CLI guide](docs/en/guide/cli.md) for
configuration, progress and explicit resume.

## Platform development

Use Node.js 24 (see `.node-version`) and install dependencies from the repository root:

```sh
npm ci
npm run platform:dev
npm run platform:check
npm run platform:build
```

The SvelteKit/TypeScript app lives in `apps/platform` and targets Cloudflare Workers.
See the [platform README](apps/platform/README.md) and [approved design](apps/platform/design/README.md).
Shared TypeScript compiler options live in the root `tsconfig.base.json`.

## Development

```sh
npm install

npm run docs:dev      # dev server
npm run docs:build    # build the site
npm test              # build + end-to-end check
```

`npm test` builds the site and then opens it in a real browser — playwright driving
your system Chrome, no browser download — and asserts that every diagram rendered,
no resource 404s, and the console stays clean.

> On machines where esbuild's temp directory is blocked by a local security policy,
> run `TMPDIR=.tmp npm run docs:build` instead.

## License

[MIT](LICENSE)
