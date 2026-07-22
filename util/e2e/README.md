# Live text-mode e2e tests

These tests exercise the real `gua connect <id>` TTY path against a running
Guacamole deployment. They are not part of `cargo test` because they require a
patched `guacd`, a reachable `guacamole-client`, credentials, and a live SSH
connection with `text-output=true`.

## SSH text-mode suite

`text-mode-ssh-live.py` logs into Guacamole, opens a text-output-enabled SSH
connection through the WebSocket tunnel, and drives the local PTY with
`pexpect`.

It validates:

- shell prompt is reached;
- stdout command output returns through the `STDOUT` pipe;
- `pwd` returns the expected remote directory;
- ANSI bytes are passed through;
- high-output stdout floods continue through the `STDOUT` pipe while the CLI drains and ACKs blobs promptly;
- Ctrl-A reaches readline;
- Ctrl-C interrupts a foreground process;
- multiline stdin reaches `cat` and Ctrl-D exits it;
- bracketed paste reaches the TUI paste-event path;
- Ctrl-] exits the local `gua connect` loop;
- SIGINT/SIGTERM process-interrupt probes exit cleanly through terminal cleanup.

Example against the current cli-enabler test VM:

```sh
cargo build
GUA_E2E_SERVER=http://10.2.0.186:8080/guacamole \
GUA_E2E_USERNAME=guacadmin \
GUA_E2E_PASSWORD=guacadmin \
GUA_E2E_CONNECTION_ID=4 \
util/e2e/text-mode-ssh-live.py
```

Useful overrides:

- `GUA_E2E_BIN` / `--gua`: path to the `gua` binary.
- `GUA_E2E_PROMPT` / `--prompt`: regex for the remote shell prompt.
- `GUA_E2E_EXPECTED_PWD` / `--expected-pwd`: expected `pwd` output.
- `GUA_E2E_LOG` / `--log`: transcript path.
- `GUA_E2E_TIMEOUT` / `--timeout`: pexpect timeout.
- `GUA_E2E_RESIZE_ROWS` / `--resize-rows` and `GUA_E2E_RESIZE_COLS` / `--resize-cols`: local PTY size used for resize propagation checks.
- `GUA_E2E_SKIP_RESIZE` / `--skip-resize`: skip resize assertions.
- `GUA_E2E_FLOOD_LINES` / `--flood-lines`: number of remote lines emitted for the high-output drain/backpressure assertion.
- `GUA_E2E_SKIP_FLOOD` / `--skip-flood`: skip the high-output drain/backpressure assertion.
- `GUA_E2E_SKIP_SIGNAL_PROBES` / `--skip-signal-probes`: skip the extra SIGINT/SIGTERM clean-exit probes.

For security, provide the Guacamole password with `GUA_E2E_PASSWORD`; avoid CLI
password arguments because they can be visible in process listings. The harness
also passes the password to `gua login` via `GUA_PASSWORD`, not `--password`.

Text resize notes: `gua-tui` prefers the terminal's reported pixel dimensions
when available. Many Unix PTYs report zero pixels, so it falls back to cell
geometry. Override fallback geometry with `GUA_TUI_CELL_WIDTH_PX`,
`GUA_TUI_CELL_HEIGHT_PX`, and `GUA_TUI_DPI` to match the server-side guacd
terminal font metrics when exact remote `stty size` fidelity matters.

The script uses a temporary `GUA_CONFIG` and `GUA_TOKEN_STORE_DIR`, so it does
not mutate the operator's normal guacamole-cli profile/token state.

## Optional chrome smoke

The live suite intentionally exercises the default raw/passthrough path. For a
manual chrome smoke, run a short session with `--chrome`, confirm that the local
status line appears at the bottom of the terminal while the remote prompt remains
usable, then exit with Ctrl-]:

```sh
GUA_CONFIG=/tmp/gua-mvp-config.toml \
GUA_TOKEN_STORE_DIR=/tmp/gua-mvp-tokens \
target/debug/gua --server http://10.2.0.186:8080/guacamole connect 4 --chrome
```

Use `--raw` to force pure passthrough; this is still the default mode used by
automated e2e tests.

## IPMI SOL control foundation

Issue #49 adds client-side protocol support for IPMI SOL's split-channel model:
SOL console bytes use the same `STDOUT` text-output path as SSH/telnet/k8s, while
structured power/status/SEL messages use a JSON `ipmi-control` pipe. The current
live SSH harness does not require an IPMI server, but unit tests cover:

- recognizing the server-opened `ipmi-control` pipe;
- parsing `state`, `result`, and `sel` JSON messages;
- serializing command JSON;
- command confirmation tiers;
- `gua connect --ipmi-control` fallback behavior where Ctrl-] is sent to the
  server-rendered fallback menu and Ctrl-5 remains the local escape.

When an IPMI-capable guacd test target is available, add a live smoke similar to
`text-mode-ssh-live.py` that verifies SOL console output plus one non-destructive
control round-trip (`refresh-status` or `read-sel`).
