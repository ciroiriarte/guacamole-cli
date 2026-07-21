#!/usr/bin/env python3
"""Live e2e test for guacamole-cli text-mode SSH sessions.

This script intentionally exercises the real `gua connect <id>` TTY path using
pexpect. It requires a reachable Guacamole webapp with a text-output-enabled SSH
connection and valid credentials. It creates an isolated temporary gua config and
token store, logs in, opens the connection, drives shell/readline/cat behavior,
and asserts the expected output.
"""

from __future__ import annotations

import argparse
import os
import pathlib
import signal
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass

try:
    import pexpect
except ImportError as exc:  # pragma: no cover - depends on operator env
    raise SystemExit(
        'Missing dependency: install Python package "pexpect" to run live e2e tests.'
    ) from exc


DEFAULT_PROMPT = r"tester@guac-build:~\$"


@dataclass
class E2EResult:
    checks: list[str]
    log_path: pathlib.Path


def env_default(name: str, default: str | None = None) -> str | None:
    value = os.environ.get(name)
    return value if value not in (None, "") else default


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run live guacamole-cli text-mode SSH e2e assertions."
    )
    parser.add_argument(
        "--gua",
        default=env_default("GUA_E2E_BIN", "target/debug/gua"),
        help="Path to gua binary (default: %(default)s or GUA_E2E_BIN).",
    )
    parser.add_argument(
        "--server",
        default=env_default("GUA_E2E_SERVER"),
        required=env_default("GUA_E2E_SERVER") is None,
        help="Guacamole base URL, e.g. http://host:8080/guacamole (or GUA_E2E_SERVER).",
    )
    parser.add_argument(
        "--username",
        default=env_default("GUA_E2E_USERNAME", "guacadmin"),
        help="Guacamole username (default: %(default)s or GUA_E2E_USERNAME).",
    )
    parser.add_argument(
        "--password",
        default=None,
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--connection-id",
        default=env_default("GUA_E2E_CONNECTION_ID"),
        required=env_default("GUA_E2E_CONNECTION_ID") is None,
        help="Text-output-enabled SSH connection id (or GUA_E2E_CONNECTION_ID).",
    )
    parser.add_argument(
        "--prompt",
        default=env_default("GUA_E2E_PROMPT", DEFAULT_PROMPT),
        help=f"Regex for the remote shell prompt (default: {DEFAULT_PROMPT!r}).",
    )
    parser.add_argument(
        "--expected-pwd",
        default=env_default("GUA_E2E_EXPECTED_PWD", "/home/tester"),
        help="Expected output of pwd inside the SSH session.",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=int(env_default("GUA_E2E_TIMEOUT", "25")),
        help="pexpect timeout in seconds (default: %(default)s).",
    )
    parser.add_argument(
        "--log",
        default=env_default("GUA_E2E_LOG", "/tmp/gua-text-mode-ssh-live.log"),
        help="PTY transcript path (default: %(default)s or GUA_E2E_LOG).",
    )
    parser.add_argument(
        "--resize-rows",
        type=int,
        default=int(env_default("GUA_E2E_RESIZE_ROWS", "33")),
        help="PTY rows to apply for resize propagation check (default: %(default)s).",
    )
    parser.add_argument(
        "--resize-cols",
        type=int,
        default=int(env_default("GUA_E2E_RESIZE_COLS", "111")),
        help="PTY columns to apply for resize propagation check (default: %(default)s).",
    )
    parser.add_argument(
        "--skip-resize",
        action="store_true",
        default=env_default("GUA_E2E_SKIP_RESIZE", "").lower() in {"1", "true", "yes"},
        help="Skip dynamic PTY resize propagation assertion.",
    )
    parser.add_argument(
        "--skip-signal-probes",
        action="store_true",
        default=env_default("GUA_E2E_SKIP_SIGNAL_PROBES", "").lower()
        in {"1", "true", "yes"},
        help="Skip SIGINT/SIGTERM process-interrupt probes.",
    )
    parser.add_argument(
        "--keep-state",
        action="store_true",
        help="Keep temporary gua config/token directory for debugging.",
    )
    return parser.parse_args()


def redact_cmd(cmd: list[str]) -> str:
    redacted: list[str] = []
    hide_next = False
    for arg in cmd:
        if hide_next:
            redacted.append("<redacted>")
            hide_next = False
            continue
        redacted.append(arg)
        if arg in {"--password", "-p"}:
            hide_next = True
    return " ".join(redacted)


def run_checked(cmd: list[str], env: dict[str, str]) -> None:
    completed = subprocess.run(cmd, env=env, text=True, capture_output=True)
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {redact_cmd(cmd)}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


def tail(path: pathlib.Path, limit: int = 5000) -> str:
    try:
        data = path.read_text(errors="replace")
    except FileNotFoundError:
        return ""
    return data[-limit:]


def login(gua: str, server: str, username: str, password: str, env: dict[str, str]) -> None:
    run_checked([gua, "--server", server, "config", "set", "server", server], env)
    login_env = env | {"GUA_USERNAME": username, "GUA_PASSWORD": password}
    run_checked(
        [
            gua,
            "--server",
            server,
            "login",
            "--username",
            username,
        ],
        login_env,
    )


def expect_prompt(child: "pexpect.spawn", prompt: str, timeout: int) -> None:
    child.expect(prompt, timeout=timeout)


def remote_stty_size(child: "pexpect.spawn", prompt: str, timeout: int) -> tuple[int, int]:
    child.sendline("stty size")
    child.expect(r"([0-9]+) ([0-9]+)", timeout=timeout)
    rows = int(child.match.group(1))
    cols = int(child.match.group(2))
    expect_prompt(child, prompt, timeout)
    return rows, cols


def probe_signal_exit(
    args: argparse.Namespace, env: dict[str, str], signum: signal.Signals
) -> str:
    cmd = f"{args.gua} --server {args.server} connect {args.connection_id}"
    child = pexpect.spawn(
        cmd,
        encoding="utf-8",
        timeout=args.timeout,
        dimensions=(24, 100),
        env=env,
    )
    try:
        expect_prompt(child, args.prompt, args.timeout)
        os.kill(child.pid, signum)
        child.expect(pexpect.EOF, timeout=args.timeout)
        return signum.name.lower() + "_exit"
    finally:
        if child.isalive():
            child.close(force=True)


def run_session(args: argparse.Namespace, env: dict[str, str]) -> E2EResult:
    log_path = pathlib.Path(args.log)
    log_path.parent.mkdir(parents=True, exist_ok=True)
    checks: list[str] = []

    cmd = f"{args.gua} --server {args.server} connect {args.connection_id}"
    child = pexpect.spawn(
        cmd,
        encoding="utf-8",
        timeout=args.timeout,
        dimensions=(24, 100),
        env=env,
    )
    with log_path.open("w", encoding="utf-8", errors="replace") as log:
        child.logfile = log
        try:
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("prompt")

            if not args.skip_resize:
                initial_size = remote_stty_size(child, args.prompt, args.timeout)
                child.setwinsize(args.resize_rows, args.resize_cols)
                # Give the CLI loop time to observe SIGWINCH / terminal_size()
                # and forward a Guacamole `size` instruction to guacd/SSH.
                time.sleep(1.0)
                resized_size = remote_stty_size(child, args.prompt, args.timeout)
                if resized_size == initial_size:
                    raise AssertionError(
                        f"remote PTY size did not change after local resize: {initial_size}"
                    )
                checks.append(
                    f"resize_propagated_{initial_size[0]}x{initial_size[1]}_to_"
                    f"{resized_size[0]}x{resized_size[1]}"
                )

            marker = "GUAC_CLI_PHASE2_STDOUT_OK"
            child.sendline(f"printf {marker}")
            child.expect(marker, timeout=args.timeout)
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("stdout_roundtrip")

            child.sendline("pwd")
            child.expect(args.expected_pwd, timeout=args.timeout)
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("pwd")

            child.sendline('printf "ANSI_RED=\\033[31mRED\\033[0m DONE\\n"')
            child.expect("ANSI_RED=", timeout=args.timeout)
            child.expect("RED", timeout=args.timeout)
            child.expect("DONE", timeout=args.timeout)
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("ansi_passthrough")

            # Ctrl-A should reach readline as beginning-of-line, allowing us to
            # comment out a side-effect command before Enter.
            child.sendline("rm -f /tmp/gua_phase2_ctrl_a_bad")
            expect_prompt(child, args.prompt, args.timeout)
            child.send("echo BAD_EXEC_MARKER > /tmp/gua_phase2_ctrl_a_bad")
            child.sendcontrol("a")
            child.send("#")
            child.sendline("")
            expect_prompt(child, args.prompt, args.timeout)
            child.sendline("test ! -e /tmp/gua_phase2_ctrl_a_bad && echo CTRL_A_OK")
            child.expect("CTRL_A_OK", timeout=args.timeout)
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("ctrl_a_readline")

            child.sendline("sleep 30")
            child.sendcontrol("c")
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("ctrl_c_interrupt")

            child.sendline("cat")
            child.sendline("PASTE_LINE_ONE")
            child.sendline("PASTE_LINE_TWO")
            child.expect("PASTE_LINE_ONE", timeout=args.timeout)
            child.expect("PASTE_LINE_TWO", timeout=args.timeout)
            child.sendcontrol("d")
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("stdin_multiline_and_ctrl_d")

            child.sendline("cat >/tmp/gua_phase2_bracketed_paste")
            child.send("\x1b[200~BRACKETED_PASTE_ONE\nBRACKETED_PASTE_TWO\n\x1b[201~")
            child.sendcontrol("d")
            expect_prompt(child, args.prompt, args.timeout)
            child.sendline("grep -qx BRACKETED_PASTE_TWO /tmp/gua_phase2_bracketed_paste && echo BRACKETED_PASTE_OK")
            child.expect("BRACKETED_PASTE_OK", timeout=args.timeout)
            expect_prompt(child, args.prompt, args.timeout)
            checks.append("bracketed_paste_event")

            child.sendcontrol("]")
            child.expect(pexpect.EOF, timeout=args.timeout)
            checks.append("local_escape_exit")

        finally:
            child.logfile = None
            if child.isalive():
                child.close(force=True)

    return E2EResult(checks=checks, log_path=log_path)


def main() -> int:
    args = parse_args()
    password = env_default("GUA_E2E_PASSWORD")
    if args.password:
        print(
            "warning: --password is insecure and deprecated for this live e2e harness; "
            "use GUA_E2E_PASSWORD instead",
            file=sys.stderr,
        )
        password = args.password
    if password is None:
        print("GUA_E2E_PASSWORD is required", file=sys.stderr)
        return 2

    gua_path = shutil.which(args.gua) if os.path.sep not in args.gua else args.gua
    if not gua_path or not pathlib.Path(gua_path).exists():
        print(f"gua binary not found: {args.gua}", file=sys.stderr)
        return 2
    args.gua = str(pathlib.Path(gua_path).resolve())

    with tempfile.TemporaryDirectory(prefix="gua-phase2-e2e-") as tmp:
        tmp_path = pathlib.Path(tmp)
        env = os.environ.copy() | {
            "GUA_CONFIG": str(tmp_path / "config.toml"),
            "GUA_TOKEN_STORE_DIR": str(tmp_path / "tokens"),
        }
        try:
            login(args.gua, args.server, args.username, password, env)
            result = run_session(args, env)
            if not args.skip_signal_probes:
                for signum in (signal.SIGINT, signal.SIGTERM):
                    result.checks.append(probe_signal_exit(args, env, signum))
        except Exception as exc:  # noqa: BLE001 - CLI test runner should print transcript context
            print(f"PHASE2_E2E_FAIL {type(exc).__name__}: {exc}", file=sys.stderr)
            print("--- transcript tail ---", file=sys.stderr)
            print(tail(pathlib.Path(args.log)), file=sys.stderr)
            if args.keep_state:
                keep = pathlib.Path(tempfile.mkdtemp(prefix="gua-phase2-e2e-failed-"))
                shutil.copytree(tmp_path, keep / "state", dirs_exist_ok=True)
                print(f"kept state at {keep / 'state'}", file=sys.stderr)
            return 1

        print("PHASE2_E2E_PASS " + ",".join(result.checks))
        print(f"transcript={result.log_path}")
        if args.keep_state:
            keep = pathlib.Path(tempfile.mkdtemp(prefix="gua-phase2-e2e-kept-"))
            shutil.copytree(tmp_path, keep / "state", dirs_exist_ok=True)
            print(f"kept state at {keep / 'state'}")
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
