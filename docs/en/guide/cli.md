# Command-line client

`q2q` builds the `q2q` executable. It uses the Rust SDK to publish, download,
and resume files; Track and Punch are operated separately. See [Deploy](./deploy.md)
for network setup.

## Install

From the repository root:

```sh
cargo install --path crates/q2q --locked
q2q --help
```

## Publish

```sh
q2q --track https://track.example.net --data-dir ./publisher-state \
  publish ./model.bin --seed ./model.q2q
```

The source is prepared and announced before the seed is exported. The seed output
must not already exist; publication stops and withdraws if export fails. Its parent
directory must exist. Send the `.q2q` seed to recipients and keep the publisher
running. The seed is a binary Q2Q document, not JSON or a magnet URL.

Ctrl-C stops serving and waits for cleanup and withdrawal. Unix SIGTERM follows
the same path. If a signal arrives during initialization (such as hashing or
connecting), the command finishes that operation and closes the acquired handle.
A failed withdrawal is reported; a remote holding may remain until its TTL expires.
A crash or forced kill cannot guarantee withdrawal.

## Download and resume

Run the receiver with a separate state directory:

```sh
q2q --data-dir ./receiver-state download ./model.q2q --output ./received.bin

# After an interruption, retain both received.bin and received.bin.q2q.resume.
q2q --data-dir ./receiver-state download ./model.q2q \
  --output ./received.bin --resume
```

`--output` is an exact filename. Its parent directory must exist. A normal download
refuses an existing destination; `--resume` explicitly revalidates an existing SDK
destination and its receipt file. Success means every block has been verified and
the destination synchronized. Downloading does not automatically share blocks or
continue seeding after completion.

The Track URL comes from `--track`, then `Q2Q_TRACK_URL`, then the first Track
in the seed. A URL is required even for a complete offline resume, although that
resume needs no network connection. The CLI does not try successive Track URLs.
CDN fallback follows the SDK's seed metadata and verification rules.

## Configuration and output

| Option | Environment / default |
| --- | --- |
| `--track` | `Q2Q_TRACK_URL`; downloading may use the seed's first Track |
| `--data-dir` | `Q2Q_DATA_DIR`; `./.q2q` |
| `--identity` | `Q2Q_IDENTITY_FILE`; `<data-dir>/identity` |
| `--download-peers` | 4; accepts 1–16 |
| `--upload-peers` | 4; accepts 1–16 |
| `--upload-rate` | Unset means unlimited; positive encoded piece bytes/second |
| `--quiet` | Suppress periodic progress and informational messages |

Options may appear before or after the subcommand. Explicit options override
environment variables. The state directory and persistent private identity are
created as needed. Do not share one identity between simultaneously running
clients; choose separate state directories and identities for those clients.

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
magnet metadata fetching, or automatic background seeding.
