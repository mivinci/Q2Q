# Q2Q

<p align="center">
  <img src="docs/public/logo.svg" width="200" alt="Q2Q">
</p>

An open-source P2P network framework for large-file distribution and audio/video
streaming (VOD and live) — faster transfers, less CDN bandwidth.

Publishers seed from their own machine and announce to the network — no backend
of their own required. The framework ships Track (discovery), Punch (signaling)
and STUN/TURN (hole punching); those are operated by whoever runs the network,
while the trust placed in them is kept minimal: a fully malicious Track or Punch
cannot alter content, only eavesdrop or refuse service.

- Website: <https://le0.me/Q2Q/>
- Design: [`docs/spec.md`](docs/spec.md)

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
