# guacamole-server patches

This directory is the home for patches to [`guacamole-server`](https://github.com/apache/guacamole-server)
that `guacamole-cli` depends on. Patches live here until implemented and (optionally) upstreamed.

## Patch index

| Patch | Status | Tracking issue |
| --- | --- | --- |
| `text-output` — opt-in raw-text output mode for terminal protocols | **Planned** | [ciroiriarte/guacamole-server#3](https://github.com/ciroiriarte/guacamole-server/issues/3) |

---

## `text-output` — raw-text output mode for terminal protocols

**Goal:** let guacd deliver the exact remote PTY byte stream (raw ANSI) over a Guacamole `pipe`
stream for SSH / telnet / Kubernetes, so the CLI can present a *true* in-terminal session instead
of decoding rasterized glyphs.

### Design

- New **opt-in connection parameter** `text-output` (default `false`).
- When enabled, tee remote PTY output to an outbound `pipe` stream (`text/plain`, e.g. `STDOUT`),
  reusing the existing `guac_terminal_pipe_stream_open()` / `..._write()` / `..._flush()` in
  `src/terminal/terminal.c` (the same machinery `guacctl` uses).
- Use **tee mode** (`GUAC_TERMINAL_PIPE_INTERPRET_OUTPUT | GUAC_TERMINAL_PIPE_AUTOFLUSH`) so the
  graphical display keeps working for browser clients — output goes to *both* display and pipe.
- Inbound STDIN already works via the existing `pipe_handler` →
  `src/terminal/terminal-stdin-stream.c`. No new input path.

### Backwards compatibility

Fully backwards compatible: additive, opt-in, default off. No new protocol instructions, no DB
schema change, no web-app change, no ABI change. Tee mode preserves the browser display.

### Security considerations

No new network surface (no new port, no auth bypass; rides the authenticated tunnel, admin-gated).
Same information as already shown graphically, but machine-readable plaintext. Must:

1. **Honor `disable-copy`** — it is effectively a copy/exfil channel; block it when copy is disabled.
2. **Not log/record pipe contents by default** — output may contain echoed secrets/tokens.

### How patches will be stored here

Once implemented, the patch will be committed as a `*.patch` file (e.g.
`0001-add-text-output-mode.patch`, generated via `git format-patch`) alongside this document, with
the build/apply instructions. Until then, this spec and the tracking issue are the source of truth.
