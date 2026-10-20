# Command-line client

`q2q` builds the `q2q` executable. It uses the Rust SDK to publish, download,
and resume files; Track and Punch are operated separately. See [Deploy](./deploy.md)
for network setup.

## Install

Versioned releases will include prebuilt archives for Linux x86_64 (glibc 2.35+),
macOS Intel and Apple silicon, and Windows x86_64 on
[GitHub Releases](https://github.com/mivinci/Q2Q/releases/latest). Download the
matching archive, unpack it, and place `q2q` (or `q2q.exe`) on your `PATH`.

To build from source, run this from the repository root:

```sh
cargo install --path crates/q2q --locked
q2q --help
```

## Connection diagnostics

```sh
q2q --verbose publish ./file.bin.q2q
q2q --verbose download ./file.bin.q2q
```

The global `--verbose` flag writes diagnostics to stderr: TURN enablement, server
endpoint and credential validity, ICE candidate gathering and exchange
(host/srflx/relay, protocol and address), WebRTC connection states, channel opening,
and handshake failures. Negotiation timeouts include connection and channel readiness.
Correlate logs using peer and connection IDs. Logs contain public/private network
addresses; review them before sharing. TURN passwords/usernames, signalling tickets
and full SDP are excluded. A relay candidate does not prove the selected path uses
TURN. Once channels open, `Selected ICE path at channel establishment` reports
`path="turn"` if either selected candidate is relay, otherwise `path="direct"`,
with both candidate types, protocols, addresses and ports. This is an establishment
snapshot, not continuous path monitoring. Unavailable/failed lookups are reported
without interrupting the transfer or guessing a direct path. TURN traffic statistics
are not reported. Diagnostics suspend the shared progress renderer while writing,
so live progress does not overwrite log lines.
Diagnostics are off by default. `--quiet --verbose` hides progress while retaining
diagnostics; stdout result formatting is unchanged.

## Build a seed offline

```sh
q2q seed ./file.bin
q2q seed ./file.bin --output ./share.q2q
```

This hashes the local file and writes a binary `.q2q` seed, then exits. It creates
no identity/state directory and contacts no Track or Punch. The default output is
`<FILE>.q2q`; existing outputs are never overwritten. `--track` / `Q2Q_TRACK_URL`
sets a discovery hint in the seed; otherwise the official default is written.
Writing that hint does not publish the file. Progress goes to stderr and the
successful seed path to stdout; `--quiet` suppresses progress.

## Publish

```sh
q2q publish ./model.bin
# Defaults to ./model.bin.q2q; pass --seed for a custom path:
q2q publish ./model.bin --seed ./model.q2q
```

Without `--seed`, the seed path is the source filename with `.q2q` appended, such as
`./model.bin.q2q`. Use `--seed` to choose a custom path. The source is prepared and
announced before the seed is exported. The seed output must not already exist;
publication stops and withdraws if export fails. Its parent
directory must exist. Send the `.q2q` seed to recipients and keep the publisher
running. The seed is a binary Q2Q document, not JSON or a magnet URL.
The seed publicly exposes the Track URL used by the publisher, which is also
the downloader's default. To avoid distributing credentials, publishing refuses
Track URLs that contain a username, password, query parameters or a fragment.

Ctrl-C stops serving and waits for cleanup and withdrawal. Unix SIGTERM follows
the same path. If a signal arrives during initialization (such as hashing or
connecting), the CLI reports source hashing progress, finishes initialization,
then closes the acquired handle. Further signals are ignored while it finishes
initialization and cleanup.
A failed withdrawal is reported; a remote holding may remain until its TTL expires.
A crash or forced kill cannot guarantee withdrawal.

### Publish an existing seed

```sh
q2q publish ./file.bin.q2q
q2q publish ./share.q2q --file ./renamed-file.bin
q2q publish ./share.q2q --file ./renamed-file.bin --seed ./export.q2q
```

A `.q2q` input (case-insensitive extension), or an input accompanied by `--file`,
is an existing seed. By default the source is the safe `info.name` beside the seed;
`--file` selects a source relative to the current working directory. Missing/unsafe
names require `--file`. The entire local file is checked against its length, block
size and Merkle root before announcing. Renamed files, custom block sizes and
unknown info fields preserve the original content identity.

The input seed is never rewritten. Without `--seed`, stdout prints its path; an
explicit `--seed` exports a new file and refuses existing destinations. Track
selection is explicit parameter/environment, the first usable seed Track, then
the official default. The in-memory/exported seed includes the selected Track;
share that new export when recipients need an overridden Track hint. For raw content
named `*.q2q`, first run `q2q seed`, then publish the resulting seed. Keep the
publisher running to serve file bytes.

## Magnet links

Create a Q2Q magnet URL locally from an existing seed:

```sh
q2q magnet ./model.bin.q2q
q2q download 'magnet:?xt=urn:sha256:<64-hex-info-hash>&tr=<percent-encoded-Track-URL>'
```

Quote links in the shell because `&` separates commands. The format uses Q2Q info
hashes, not BitTorrent `btih`/`btmh`. `magnet` prints one URL to stdout without
creating an identity or connecting to a service; `publish` still prints the seed path.

Downloads try the explicit `--track` / `Q2Q_TRACK_URL`, valid `tr` hints, then the
official Track, deduplicating URLs. With no `tr`, the default official Track still
works when it knows an online metadata source. Metadata discovery has a 60-second
budget across CLI Track attempts. The CLI authenticates a Peer, verifies its info
against `xt`, and only then opens the output. `dn` is a display hint and never
chooses the output path. A safe verified `info.name` is used in the current directory;
otherwise the full info hash becomes the filename. `--output` overrides it.

Verified metadata is cached under `<Q2Q_HOME>/metadata/<info-hash>.q2q` and checked
again when read. Keep this cache with the destination and checkpoints for automatic
resume. A complete download can finish revalidation offline without opening an
identity or Track session. Metadata discovery can be interrupted immediately;
`--quiet` suppresses its spinner and progress messages.

The initial implementation fetches info from online complete or partial Peers.
Track stores no metadata. CDN URLs and leaf/proof lists are seed-envelope fields,
so magnets do not recover CDN fallback: a CDN-only file with no online Peer still
requires its `.q2q` seed. Raw info is limited to 8 KiB, links to 16 KiB; noncanonical,
wrong-hash and unsupported metadata are rejected before any destination is created.

## Download and resume

The CLI stores identity and checkpoints in the platform user state directory by
default. Set `Q2Q_HOME` to choose another root:

```sh
Q2Q_HOME=./receiver-state q2q download ./model.q2q

# After an interruption, retain the destination and receiver-state directory,
# then run the same command again to resume automatically.
Q2Q_HOME=./receiver-state q2q download ./model.q2q
```

The CLI prefers the seed's `info.name` as a filename in the current directory. If it
is missing or unsafe, the CLI derives a name from the seed filename without its
extension. Safe names must be a single filename, without path separators, control
characters or platform-reserved characters. Pass `--output` to choose an exact
destination path; its parent directory must exist. When the destination and a
valid checkpoint both exist, the CLI revalidates and resumes automatically. An
existing destination without a valid checkpoint is still an error and is never
overwritten. `--resume` explicitly requires checkpoint-based recovery. Checkpoints are stored under
`Q2Q_HOME/checkpoints`, keyed by the normalized destination path. A legacy adjacent
`.q2q.resume` file is migrated on the first resume. Success means every block has
been verified and the destination synchronized. Downloading does not automatically
share blocks or continue seeding after completion.

The preferred Track URL comes from `--track`, then `Q2Q_TRACK_URL`, then the
seed's first Track, and finally `https://track.q2q.app`. For a download, the CLI
tries that preferred URL first, then each valid Track URL from the seed in order,
and the official URL as a final fallback; duplicate URLs are skipped. It moves
to the next candidate when session setup fails or the initial peer query fails
or returns no peers. Once a Track returns peers, the transfer stays with that Track;
subsequent peer retries and CDN fallback remain within the SDK. A complete
checkpoint resume is verified locally before Track configuration or identity
initialization; Track is selected only if data still needs downloading.
CDN fallback follows the SDK's seed metadata and verification rules.

## Configuration and output

| Option | Environment / default |
| --- | --- |
| `--track` | `Q2Q_TRACK_URL`; downloading may use the seed's first Track; defaults to `https://track.q2q.app` |
| `--insecure` | Off by default; skip Track/CDN HTTPS certificate and hostname verification for debugging only |
| `Q2Q_HOME` | Platform user state directory; root for Q2Q identity and local state |
| `--identity` | `Q2Q_IDENTITY_FILE`; `<Q2Q_HOME>/identity` |
| `--download-peers` | 4; accepts 1–16 |
| `--upload-peers` | 4; accepts 1–16 |
| `--upload-rate` | Unset means unlimited; positive encoded piece bytes/second |
| `--quiet` | Suppress periodic progress and informational messages |

For temporary debugging with self-signed HTTPS or proxy certificates, use
`q2q --insecure publish ./model.bin` or `q2q --insecure download ./model.bin.q2q`.
It applies to this invocation's Track, CDN content and proof metadata HTTPS
requests, which no longer authenticate the server through its certificate.
Punch key pinning, Peer authentication and Seed/Merkle content verification
still apply. Omit the flag during normal use.

Options may appear before or after the subcommand. Explicit options override
environment variables. The state directory and persistent private identity are
created as needed. Defaults are `$XDG_STATE_HOME/q2q` (or `~/.local/state/q2q`)
on Linux/other Unix, `~/Library/Application Support/Q2Q` on macOS, and
`%LOCALAPPDATA%\Q2Q` on Windows. The CLI locks its identity while running. Give
simultaneous clients separate `Q2Q_HOME` or `Q2Q_IDENTITY_FILE` values. Existing
development data in `./.q2q` is not moved automatically; set `Q2Q_HOME=./.q2q`
to keep using that identity.

Successful seed or downloaded-file paths go to stdout, one line per command.
Progress and errors go to stderr, including in redirected/noninteractive sessions.
Publishing prints the seed path once export succeeds and continues running;
it does not mean a recipient has completed a download.

Interactive terminals display a download progress bar with percentage, verified
bytes, speed, ETA, active Peer count and HTTP request count. Publication displays
a live summary and one row per upload connection: shortened Peer ID, connection
state, sent bytes, rate and retransmitted bytes. Ended connection rows are removed.
The local Peer ID is printed at startup. Download connections also have live
rows: shortened Peer ID, state, received payload, per-attempt receive rate and
verified/persisted bytes. Reconnections receive new attempt IDs and rate baselines.
A Connecting row identifies a candidate; its identity is confirmed when the
connection authenticates. Ended attempts disappear rather than forming a history.
Receive rate includes accepted matching duplicate pieces, excludes piece headers,
control traffic and discarded old blocks, and can exceed verified-byte rate.
CDN and resumed data are not attributed to a Peer. See [download observation](./transport#download-peer-observation).

Redirected stderr and `TERM=dumb` use plain line logs without terminal control
codes. `--quiet` disables both the live display and progress logs.

Progress is sampled once per second. Download speed counts newly verified,
persisted bytes, excluding resumed bytes. Upload speed counts encoded piece bytes
accepted by the SDK, including retransmits. Neither is a network-interface byte
counter. The CLI seed input is limited to 8 MiB.

Exit codes: **0** successful completion, **1** runtime/configuration/cleanup error,
**2** command-line parsing error, **130** a handled interrupt (including Unix
SIGTERM). Help and version exit successfully. If cleanup fails after an interrupt,
the command reports the error and exits with 1.

This first version has no daemon mode, GUI, browser receiver, account service,
or automatic background seeding.
