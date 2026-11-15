# Web client

`apps/www` implements local file sharing and seed downloads without a platform login.
Sharing/P2P pages are deployed. The cross-device browser matrix is incomplete;
Mac Safari receiving an iPhone Safari share is under investigation. New diagnostics
described here belong to the development build and require deployment.

## Share a local file

Choose **Share file** (or drop an ordinary file), review its name/size and foreground
notice, then choose **Start sharing**. The SDK builds the seed and declares a source;
it does not upload file contents to platform storage. Once waiting for a receiver,
choose **Save seed** and send the `.q2q` file to the recipient. Keep the page open.

Sent bytes and connection counts come from SDK snapshots; they are not proof that a
recipient has verified/saved the complete file. Waiting for a receiver is not an error.
Pause withdraws availability, resume reannounces and stop releases resources. File
handles, original seed bytes, credentials and private keys are not persisted as share
history. Reload/close does not keep seeding; choose the file again to publish it again.
A local-sharing seed exposes the content identity/name and can be used by downloaders
on the configured network. Confirm the intended access scope before distributing it.

## Download a seed

Open/drop `.q2q`, review it locally, then choose **Start download**. The SDK owns peer
connections, HTTP fallback, verification and checkpoints, including peer-only seeds.
Missing/offline sources are distinct from corrupt content. Details show the P2P control
connection state only: online does not prove content used P2P or a particular TURN path.

## Download with a magnet link

**Download** accepts both a `.q2q` seed and a Q2Q magnet link (`magnet:?xt=urn:sha256:…`):
paste the link, fetch metadata, review, then start the download. A magnet carries only the
content identity; name, size and proof data come from verified metadata served by an
online peer. At least one peer must be online and metadata discovery has a 60-second
budget. Only `urn:sha256` is supported; BitTorrent `btih`/`btmh` links report a clear
error. CDN-only content with no online peer cannot cold-start from a magnet and still
needs the complete `.q2q` seed.

- The page connects only to its build-configured Track and ignores `tr` hints. The
  `dn` display name is never trusted or used to name the target.
- Unverified metadata is rejected and the next candidate is tried; exhausting candidates
  reports "no source", separately from connection failure or timeout.
- The verified minimal seed is cached in local IndexedDB (content hash and seed bytes
  only, no link or display name). **Paste link again** after reload prefers that cache
  instead of fetching metadata again, and a matching `.q2q` seed can also restore the
  same task.
- **Copy magnet link** in a share row's menu produces a pasteable link. If clipboard
  access is denied, a dialog shows the readonly link for manual copying. The link content
  follows the seed's Track list; the receiving page still uses only its build Track.

Completed data stays in local OPFS until **Save file** exports it. A native save picker
uses streaming export; otherwise the bounded Blob fallback is limited to 32 MiB and
reports submission to browser downloads, not confirmed operating-system durability.
After reload, history is a hint: reselect the matching seed (or re-paste the same magnet
link) so the SDK can reread and verify checkpointed bytes before continuing.

Completion does **not** start sharing. **Share this file** in the menu explicitly asks
the SDK to reverify the complete cache and publish it. Stop that publication before
deleting its cache; files already saved elsewhere are not deleted. Live partial-block
sharing is available in the SDK but is not enabled by default or exposed in this UI.

## Configuration and limits

`PUBLIC_Q2Q_TRACK_URL` is declared public/static in SvelteKit `src/env.ts` and defaults
to `https://track.q2q.app`. An empty string disables networking while retaining HTTP.
Tests may use HTTP loopback; public Track must use HTTPS. Never place tokens in this
public configuration. The page does not construct Punch URLs or silently follow a
seed's Track list; explicit discovery and authenticated WSS are SDK responsibilities.

Local persistent Chromium/WebKit profiles verify real seed export, browser P2P file
hashes, lifecycle, explicit cache sharing/protection, language/narrow layout and existing
HTTP recovery. Chromium ↔ native file flows through the page pass; WebKit/native ICE
remains unsupported by current acceptance and is skipped. This is not Safari.app,
mobile background, cross-NAT, forced TURN or arbitrary-size acceptance. Public SDK
success does not mean the Web application itself has been deployed.


## Disconnects and retry

When no source is online, confirm the sender keeps the page open and actively sharing,
then choose **Retry** on the download. Reload recovery requires the matching seed again;
the SDK revalidates committed bytes rather than trusting matching filenames.

A disconnected publication reports failure; it is not an automatic background reconnect
job. After network/service recovery, select the source file and explicitly share again.
Save seed stays disabled if preparation never produced a usable seed; remove the failed
task before starting again. Reload does not restore a share as falsely online. Remote
records can remain until their TTL expires.

These paths were verified with isolated local services. Cross-device, cross-NAT and
forced TURN acceptance are deferred, not passed. Retry is not a connectivity guarantee
for every network.


## Inspect a failed connection (development build)

Open **Details → Transfer details → Connection diagnostics** on a failed task.
Copy the error code, stage, ICE/connection/channel states and candidate counts for
troubleshooting. Do not send full SDP, cookies, tickets or TURN credentials.
Diagnostics exclude addresses and credentials, are not uploaded automatically and
are not guaranteed to survive refresh. A network failure is not necessarily CORS;
an online control connection does not prove WebRTC connected. A shared Wi-Fi or
an Edge success does not establish Safari-to-Safari compatibility.
