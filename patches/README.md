# guacamole-server patches

This directory is the home for patches to [`guacamole-server`](https://github.com/apache/guacamole-server)
that `guacamole-cli` depends on. Patches live here until implemented and (optionally) upstreamed.

## Patch index

| Patch | Status | Tracking issue |
| --- | --- | --- |
| [`0001-add-text-output-mode.patch`](0001-add-text-output-mode.patch) — opt-in raw-text output modes for terminal protocols | **Exported + implemented** (on `feature/3-text-output-mode`) | [ciroiriarte/guacamole-server#3](https://github.com/ciroiriarte/guacamole-server/issues/3) |

---

## `text-output` — raw-text output modes for terminal protocols

**Goal:** let guacd deliver the exact remote PTY byte stream (raw ANSI) over a Guacamole `pipe`
stream for SSH / telnet / Kubernetes, so the CLI can present a *true* in-terminal session instead
of decoding rasterized glyphs.

### Design

- New **opt-in connection parameter** `text-output` (default `false`). Supported enabled values:
  - `text-output=true`: tee mode. Raw PTY output is sent to the text pipe while the normal graphical
    terminal display continues to render for browser clients.
  - `text-output=raw`: headless/raw mode. Raw PTY output is sent to the text pipe and the graphical
    terminal render path is skipped. This is the preferred mode for a dedicated CLI consumer.
- When enabled, guacd opens an outbound `pipe` stream named **`STDOUT`** with mimetype
  **`application/octet-stream`** (the payload is raw bytes carried in base64 `blob` instructions, so
  it is binary-safe; the mimetype is advisory and the client decodes the bytes itself).
- The stream is allocated on the **connection owner's user socket** (via `guac_user_alloc_stream()`),
  not on the broadcast client socket. This keeps raw output scoped to the owner and, critically,
  gives the stream an ack-routable user stream index.
- The tee is taken **at the protocol source** — in each protocol's PTY read path (SSH/telnet/k8s),
  *upstream of the terminal emulator* — via a **dedicated** `text_output_stream` on `guac_terminal`
  (API `guac_terminal_text_output_{open,write,flush,close}` in `src/terminal/terminal.h`),
  independent of the `guacctl` pipe machinery. This is deliberate: the existing `guac_terminal_echo`
  path strips `ESC`/CSI/OSC sequences, which would defeat a faithful raw stream.
- Output is buffered in 6048-byte chunks. Tee mode flushes at terminal frame boundaries or when full;
  raw/headless mode flushes immediately because there is no graphical frame cycle.
- Clients **must send Guacamole `ack` instructions for each received `blob`**, and must do so
  promptly — acknowledge on receipt rather than after rendering, so that a slow or blocked local
  terminal cannot stall the ack stream. Guacd bounds the unacknowledged backlog at **256 KB** (and at
  most 256 outstanding blobs); the byte bound is the one that matters in practice, since raw mode
  emits one blob per PTY read and those blobs are often only a few bytes.
- **The response to exceeding that bound differs by mode**, and a client author must account for both:
  - `text-output=true` (tee): buffered output is **dropped** and the session continues. Delivery is
    best-effort — output is lost silently rather than blocking the remote PTY/read loop, which would
    also stall co-attached browser users.
  - `text-output=raw`: the **connection is aborted** with `SERVER_ERROR` and the message
    `text-output consumer is not keeping up`. Raw mode is the sole output channel and is
    byte-oriented, so guacd fails fast rather than delivering a silently-corrupted stream. Clients
    should surface this status distinctly from an ordinary disconnect.
- Inbound STDIN already works via the existing `pipe_handler` →
  `src/terminal/terminal-stdin-stream.c`. No new input path.

### Backwards compatibility

Fully backwards compatible: additive, opt-in, default off. No new protocol instructions (reuses
`pipe`/`blob`/`end`/`ack`), no DB schema change, no web-app change, no ABI break (only additive public
API: the new `guac_terminal_text_output_*` symbols). Tee mode preserves the browser display; raw mode
is opt-in for CLI-only connections.

### Security considerations

No new network surface (no new port, no auth bypass; rides the authenticated tunnel, admin-gated).
Same information as already shown graphically, but machine-readable plaintext. Must:

1. **Honor `disable-copy`** — it is effectively a copy/exfil channel; block it when copy is disabled.
2. **Not log/record pipe contents by default** — output may contain echoed secrets/tokens.

### Patch artifact

The feature is implemented on the `feature/3-text-output-mode` branch of the server fork and is
exported here as [`0001-add-text-output-mode.patch`](0001-add-text-output-mode.patch). The file is a
single `git format-patch --stdout` mailbox containing the text-output commit series from the server
fork.

Apply it to a clean checkout of the pinned guacamole-server base with:

```sh
git am patches/0001-add-text-output-mode.patch
```

Build guacd with the normal Apache Guacamole server bootstrap/configure flow for your platform, then
run the server fork's text-output smoke/manual tests documented in
`util/manual-tests/README-text-output-e2e.md`.
