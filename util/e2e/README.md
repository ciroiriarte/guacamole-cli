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
- Ctrl-A reaches readline;
- Ctrl-C interrupts a foreground process;
- multiline stdin reaches `cat` and Ctrl-D exits it;
- Ctrl-] exits the local `gua connect` loop.

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

The script uses a temporary `GUA_CONFIG` and `GUA_TOKEN_STORE_DIR`, so it does
not mutate the operator's normal guacamole-cli profile/token state.
