# Command-line client

Use `q2q` to publish local files or download them through magnet links and `.q2q` seeds. The default discovery service is `https://track.q2q.app`, so everyday use needs no server configuration.

## Install

Download the archive for your system from [GitHub Releases](https://github.com/mivinci/Q2Q/releases/latest), unpack it, and place `q2q` (`q2q.exe` on Windows) on your `PATH`. Builds are available for Linux x86_64 (glibc 2.35+), macOS Intel / Apple silicon, and Windows x86_64.

To install from source, run this from the repository root:

```sh
cargo install --path crates/q2q --locked
q2q --help
```

## Command reference

| Task | Command | What happens |
| --- | --- | --- |
| Download a file | `q2q download <magnet-or-seed>` | Downloads and verifies the file, then exits |
| Publish and serve a file | `q2q publish <file-or-seed>` | Keeps running to serve recipients |
| Build a seed only | `q2q seed <file>` | Writes a local `.q2q` seed, then exits |
| Get a magnet link | `q2q magnet <seed>` | Prints one link locally, then exits |

Add `--help` to any command for its options, for example `q2q seed --help`.

## Download a file

Download a magnet link directly. The shortest format needs only the content hash; use the actual link supplied by the publisher:

```sh
q2q download 'magnet:?xt=urn:sha256:5caa06b9354427d3c06b3223cd17e2b2da205d12ee0865e78874e37f4d869074'
```

For a `.q2q` seed, pass the file as input:

```sh
q2q download ./video.mp4.q2q
```

Downloads go to the **current directory**, preferring the filename from the seed or verified metadata. Use `--output` (or `-o`) to select an exact destination path. Its parent directory must already exist:

```sh
q2q download ./video.mp4.q2q --output ./downloads/video.mp4
```

### Continue an interrupted download

Keep the partial file and Q2Q state directory, then run the same command again. The CLI detects the checkpoint, revalidates existing blocks, and resumes automatically:

```sh
q2q download ./video.mp4.q2q --output ./downloads/video.mp4
```

Magnet downloads also resume automatically. Use `--resume` when you explicitly require an existing checkpoint.

If a destination exists without a valid checkpoint, the CLI reports an error and preserves the file; choose another output path. Success means all content has been verified and synchronized to disk. Downloads exit on completion; run `publish` to keep sharing the file.

## Publish a file

On the device that holds the original file, run:

```sh
q2q publish ./video.mp4
```

This creates `./video.mp4.q2q` by default and keeps running after publication succeeds. Send the seed to recipients and **keep the publisher process running**. Press Ctrl-C to stop sharing. Unix SIGTERM also triggers cleanup and withdrawal.

Use `--seed` to choose a different seed path. The output must not already exist:

```sh
q2q publish ./video.mp4 --seed ./share.q2q
```

### Share a magnet link

Once published, generate a magnet link from the seed in another terminal:

```sh
q2q magnet ./video.mp4.q2q
```

Send the complete output line to recipients. With the official Track, the link can use the minimal `xt`-only format shown above. Generated links may also contain a filename hint (`dn`) and Track hints (`tr`). A magnet carries a content reference; recipients still need to discover an online source.

`magnet` reads the local seed and prints a link. The running `publish` process serves the file bytes.

## Build a seed only

Use `seed` to prepare a seed now and publish it later:

```sh
q2q seed ./video.mp4
```

This reads and hashes the local file, writes `./video.mp4.q2q`, then exits. It works offline, makes no announcement, and creates no identity or state directory. Choose a different seed path with `--output`:

```sh
q2q seed ./video.mp4 --output ./share.q2q
```

The output must not already exist. Both `seed` and `publish` produce binary `.q2q` seeds. After a seed-only operation, recipients need a publisher to come online before they can fetch its file.

### Publish an existing seed

If the original file is still beside the seed under its original name:

```sh
q2q publish ./video.mp4.q2q
```

If it has moved or been renamed, select it with `--file`:

If you see `source file not found`, the error shows the source path the CLI tried to use. A seed contains metadata, not the original file contents; make sure the source exists or use `--file` to select its actual location.

```sh
q2q publish ./share.q2q --file ./archive/renamed.mp4
```

The CLI checks that the local file matches the seed and preserves its content identity. The input seed is reused without being rewritten. To export a copy carrying a new Track hint, add `--seed ./export.q2q`.

A seed with no usable filename also requires `--file`. For raw content whose filename ends in `.q2q`, first create a seed with `seed`, then publish that generated seed.

## Configuration

### State directory and output location

`Q2Q_HOME` holds the identity, checkpoints, and magnet metadata cache. Downloaded files still default to the current directory. The installation method does not change these locations.

| System | Default `Q2Q_HOME` |
| --- | --- |
| Linux / other Unix | `$XDG_STATE_HOME/q2q`, or `~/.local/state/q2q` when unset |
| macOS | `~/Library/Application Support/Q2Q` |
| Windows | `%LOCALAPPDATA%\Q2Q` |

On Linux / macOS, choose a state directory for one invocation:

```sh
Q2Q_HOME=./receiver-state q2q download ./video.mp4.q2q --output ./video.mp4
```

In Windows PowerShell, set `$env:Q2Q_HOME = '.\receiver-state'` before running the command.

Identity and state directories are created as needed. For simultaneous clients on one device, use separate `Q2Q_HOME` values or select separate identity files with `--identity`. Resume using the original state directory and destination path.

<details>
<summary>State files and default download names</summary>

- The default identity is `<Q2Q_HOME>/identity`; checkpoints live in `<Q2Q_HOME>/checkpoints`, and magnet metadata in `<Q2Q_HOME>/metadata`. Keep the whole directory to retain resume state.
- A seed with a missing or unsafe content name falls back to the seed filename. A magnet with a missing or unsafe verified name uses the full content hash. `--output` overrides either default.
- A complete checkpoint can be revalidated offline. Magnets also need their verified metadata cache. Legacy adjacent `.q2q.resume` checkpoints are migrated on resume.
- Existing development data in `./.q2q` is not moved automatically; set `Q2Q_HOME=./.q2q` to reuse it.

</details>

### Common options

Global options may appear before or after the subcommand. Explicit options override their environment variables.

| Option | Purpose and default |
| --- | --- |
| `--track URL` | Preferred Track; also accepts `Q2Q_TRACK_URL`, defaults to the official service |
| `--identity PATH` | Identity file; also accepts `Q2Q_IDENTITY_FILE` |
| `--download-peers N` | Maximum concurrent download peers or CDN requests; default 4, range 1–16 |
| `--upload-peers N` | Maximum admitted upload connections; default 4, range 1–16 |
| `--upload-rate N` | Total upload limit in bytes/second; a positive integer, unlimited by default |
| `--quiet` | Hide progress and informational messages, retaining results and errors |
| `--verbose` | Show connection and transfer-stage diagnostics |
| `--insecure` | Skip Track/CDN HTTPS certificate and hostname checks for debugging; off by default |

For `seed`, `--output` selects the seed path; for `download`, it selects the downloaded file. `publish` uses `--seed` for seed export.

### Custom Tracks and magnet hints

```sh
q2q --track https://track.example.com publish ./video.mp4
```

`seed` can also use `--track` to write a discovery hint for later publication and downloading. The publisher's Track address is included in the shared seed, so it must not contain a username, password, query parameters, or a fragment.

A magnet's `tr` can supply a custom discovery address. Omitted schemes default to HTTPS; explicit HTTP and HTTPS are supported. Values are percent-encoded once using URL query rules. For example, use `tr=track.example.com` for HTTPS, or `tr=http%3A%2F%2Flocalhost%3A8080` for local HTTP. Generated links omit the HTTPS scheme.

An `xt`-only link uses the official Track to find sources. On a custom network, keep its `tr` hint or have the recipient pass `--track`. `dn` is an optional name hint; output filenames come from verified content information. Always quote the complete link so the shell does not interpret `&` as a command separator.

<details>
<summary>Track selection and magnet support</summary>

Downloads first try explicit `--track` / `Q2Q_TRACK_URL`, then valid seed Tracks or magnet `tr` hints, then the official URL, skipping duplicates. They move on when session setup or the initial query fails, or no usable peers are found. Once a Track yields peers, subsequent retries are handled by the SDK.

Publishing an existing seed chooses explicit configuration, the first usable seed Track, then the official URL. After overriding the address, use `--seed` to export a copy if recipients need the new hint.

Q2Q magnets use `urn:sha256` content identities, do not accept BitTorrent `btih` / `btmh`, and fetch verified metadata from online complete or partial peers. A CDN-only source with no online peer requires a `.q2q` seed because magnets do not carry the seed envelope's CDN and proof URLs. CLI magnet metadata discovery has a 60-second budget. Seed input is limited to 8 MiB, links to 16 KiB, and raw info to 8 KiB. See [Wire encoding](./wire.md) for protocol details.

</details>

## Progress and output

Successful results go to stdout: `seed`, `publish`, and `download` print one file path; `magnet` prints one link. Progress, informational messages, and errors go to stderr, so scripts can capture results independently.

Interactive terminals show transfer progress, rates, and per-peer state. Redirected stderr or `TERM=dumb` uses plain text. Download summary speed counts newly verified and persisted bytes; per-peer receive rates include valid duplicate pieces. Upload speed counts encoded piece bytes handed to WebRTC, including retransmissions. These figures differ from network-interface traffic. A publisher printing its seed path means it has started serving the file.

| Exit code | Meaning |
| --- | --- |
| 0 | Successful completion |
| 1 | Runtime, configuration, or cleanup error |
| 2 | Command-line argument error |
| 130 | Handled interruption, including Unix SIGTERM |

## Troubleshoot a connection

Enable `--verbose` on both devices and keep the full logs from the same test:

```sh
q2q --verbose publish ./video.mp4.q2q
q2q --verbose download ./video.mp4.q2q
```

Correlate records using Peer and connection IDs. Logs cover negotiation, identity verification, bitfields, block requests, retries, and failure stages. See [Connection diagnostics](./transport#connection-diagnostics) for the fields.

At connection establishment, `Selected ICE path at channel establishment` reports the selected route: `path="turn"` uses a TURN relay, and `path="direct"` uses a direct path. This is a snapshot; a relay candidate alone does not confirm the selected route. Logs omit TURN credentials but include network addresses.

Use `--quiet --verbose` to retain diagnostics and errors while hiding progress. For temporary debugging with self-signed HTTPS or proxy certificates, add `--insecure`. It applies only to that invocation; Peer authentication and content verification still run.

See [Deploy](./deploy-single-node) for service setup and [CLI releases](./cli-release.md) for the maintainer's release process.
