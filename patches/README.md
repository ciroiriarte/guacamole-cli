# guacamole-server patches

This directory is the home for patches to [`guacamole-server`](https://github.com/apache/guacamole-server)
that `guacamole-cli` depends on. Patches live here until implemented and (optionally) upstreamed.

## Patch index

| Patch | Status | Tracking issue |
| --- | --- | --- |
| `text-output` — opt-in raw-text output mode for terminal protocols | **Implemented** (on `feature/3-text-output-mode`) | [ciroiriarte/guacamole-server#3](https://github.com/ciroiriarte/guacamole-server/issues/3) |

---

## `text-output` — raw-text output mode for terminal protocols

**Goal:** let guacd deliver the exact remote PTY byte stream (raw ANSI) over a Guacamole `pipe`
stream for SSH / telnet / Kubernetes, so the CLI can present a *true* in-terminal session instead
of decoding rasterized glyphs.

### Design

- New **opt-in connection parameter** `text-output` (default `false`).
- When enabled, tee remote PTY output to an outbound `pipe` stream named **`STDOUT`** with mimetype
  **`application/octet-stream`** (the payload is raw bytes carried in base64 `blob` instructions, so
  it is binary-safe; the mimetype is advisory and the client decodes the bytes itself).
- The tee is taken **at the protocol source** — in each protocol's PTY read path (SSH/telnet/k8s),
  *upstream of the terminal emulator* — via a **dedicated** `text_output_stream` on `guac_terminal`
  (API `guac_terminal_text_output_{open,write,flush,close}` in `src/terminal/terminal.h`),
  independent of the `guacctl` pipe machinery. This is deliberate: the existing `guac_terminal_echo`
  path strips `ESC`/CSI/OSC sequences, which would defeat a faithful raw stream.
- Still a **tee**: the graphical display keeps rendering for browser clients — the raw bytes go to
  the `STDOUT` pipe *in addition to* the normal display path. Buffered (6048 B) and flushed at the
  terminal frame boundary or when full.
- Inbound STDIN already works via the existing `pipe_handler` →
  `src/terminal/terminal-stdin-stream.c`. No new input path.

### Backwards compatibility

Fully backwards compatible: additive, opt-in, default off. No new protocol instructions (reuses
`pipe`/`blob`/`end`), no DB schema change, no web-app change, no ABI break (only additive public API:
the new `guac_terminal_text_output_*` symbols). Tee mode preserves the browser display.

### Security considerations

No new network surface (no new port, no auth bypass; rides the authenticated tunnel, admin-gated).
Same information as already shown graphically, but machine-readable plaintext. Must:

1. **Honor `disable-copy`** — it is effectively a copy/exfil channel; block it when copy is disabled.
2. **Not log/record pipe contents by default** — output may contain echoed secrets/tokens.

### How patches will be stored here

The feature is implemented on the `feature/3-text-output-mode` branch of the server fork; the
authoritative source is that branch and tracking issue #3. A standalone `*.patch` artifact (e.g.
`0001-add-text-output-mode.patch`, via `git format-patch`) has **not** been exported here yet — add
it, with build/apply instructions, if/when a self-contained patch is needed for redistribution.
