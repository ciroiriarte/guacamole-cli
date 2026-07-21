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
import shutil
import subprocess
import sys
import tempfile
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
        default=env_default("GUA_E2E_PASSWORD"),
        required=env_default("GUA_E2E_PASSWORD") is None,
        help="Guacamole password (or GUA_E2E_PASSWORD).",
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
        "--keep-state",
        action="store_true",
        help="Keep temporary gua config/token directory for debugging.",
    )
    return parser.parse_args()


def run_checked(cmd: list[str], env: dict[str, str]) -> None:
    completed = subprocess.run(cmd, env=env, text=True, capture_output=True)
    if completed.returncode != 0:
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(cmd)}\n"
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
            "--password",
            password,
        ],
        login_env,
    )


def expect_prompt(child: "pexpect.spawn", prompt: str, timeout: int) -> None:
    child.expect(prompt, timeout=timeout)


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
            login(args.gua, args.server, args.username, args.password, env)
            result = run_session(args, env)
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
