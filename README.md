# guacamole-cli

A specialized **command-line / native client for [Apache Guacamole](https://guacamole.apache.org/)**.

Access remote sessions through the `guacamole-client` gateway as the **sole public entry point** — reusing Guacamole's authentication, authorization, and audit machinery — without a web browser:

- **Text protocols** (SSH, telnet, Kubernetes) as a true in-terminal experience.
- **Graphical protocols** (RDP, VNC, SPICE) rendered in a native GUI window.
- **Shared drives** mounted locally via FUSE (better UX than the browser's upload/download).
- A full **REST management CLI** (connections, users, groups, permissions, sessions, history) — in the spirit of `openstack` / [`pve-cli`](https://github.com/ciroiriarte/pve-cli).

> Status: **early design / scaffolding.** This README is the north-star architecture, not a description of shipped functionality.

## Why

`guacamole-client` (the Java web app) is the public gateway and the home of Guacamole's auth (DB/LDAP/SAML/OIDC/TOTP), permissions, and audit. Its tunnel is a *client-agnostic instruction protocol* — `guacamole-common-js` is just one (browser) implementation of it. This project is a **native implementation of that same client**, so you get Guacamole's centralized control plane with a terminal-native UX.

A plain `ssh` client **cannot** ride the gateway: guacd terminates SSH itself (libssh2) and there is no SOCKS/TCP-forward in Guacamole. The only viable path is to speak the Guacamole protocol — which is exactly what this client does.

## Architecture

```
+-------------------+        HTTPS / WSS         +------------------+        native        +--------+
|   guacamole-cli   |  <-- REST + tunnel proto -->| guacamole-client |  <-- guac proto -->  | guacd  | --> SSH/RDP/VNC/...
| (Rust, one binary)|     (sole public entry)     |   (Java gateway) |                      |        |
+-------------------+                             +------------------+                      +--------+
        |
        +-- TUI (ratatui) for SSH/telnet/k8s text
        +-- GUI window (winit/wgpu or SDL2) for RDP/VNC/SPICE
        +-- FUSE mount for drive redirection
```

### Two planes

| Plane | What | Server change? |
| --- | --- | --- |
| **Management** | REST wrapper: tokens/login, connections, groups, users, permissions, sharing profiles, active sessions, history | None |
| **Session (transport)** | `POST /api/tokens` → WebSocket tunnel → Guacamole instruction protocol | None |
| **RDP / VNC / SPICE** | native renderer of the drawing instruction set + input/audio/clipboard | None |
| **SSH / telnet / k8s (text)** | guacd emits raw PTY bytes on a `STDOUT` pipe (`application/octet-stream`) → local TTY interprets ANSI | **Small opt-in guacd patch** (see `patches/`) |
| **Copy / paste (text)** | `clipboard` streams — already real UTF-8 text | None |
| **Shared drives** | Guacamole file streams exposed as a FUSE mount | None |

The **only** server-side dependency is the text-output mode for terminal protocols, tracked as an upstream enhancement and documented under [`patches/`](./patches). Everything else is pure client-side work against the documented REST API and tunnel protocol.

## Language

**Rust.** This is a long-lived, network-facing client with a real-time graphics pipeline, a TUI, FUSE, and WebSocket streaming — a profile that wants Rust's performance, memory safety (it parses untrusted data from a public gateway), and single-static-binary distribution. Reference spec for the instruction set is `guacamole-common-js`.

## Roadmap

1. **Management CLI + transport** — REST wrapper, tunnel client, clipboard copy/paste for text protocols. *(No server changes.)*
2. **guacd text-output patch + native TUI** — real in-terminal SSH/telnet/k8s. *(See `patches/`; upstream the patch.)*
3. **Native GUI renderer** — RDP/VNC/SPICE window (the largest component).
4. **FUSE drive redirection + SSO (browser-loopback) + recording playback.**

## Proposed CLI shape

```
gua login                          # /api/tokens, browser-loopback for SSO
gua connection list|create|update|delete|share
gua session list|kill
gua connect <id>                   # SSH/telnet/k8s -> terminal (TUI)
gua connect <id> --mount ~/share   # FUSE drive redirection
gua connect <id>                   # RDP/VNC/SPICE -> native GUI window
gua record get|play <id>           # guacenc / guaclog
```

### Text-mode MVP

`gua connect <id>` currently targets SSH/telnet/Kubernetes connections that have the patched guacd
`text-output` parameter enabled (`text-output=raw` is preferred for CLI-only use). The client opens
the authenticated guacamole-client WebSocket tunnel, consumes the owner-scoped `STDOUT` pipe
(`application/octet-stream`), acknowledges each received `blob`, and writes decoded raw PTY bytes to
the local terminal. Press `Ctrl-]` to exit the local client loop.

Text input is sent as Guacamole `key` instructions. The client maps printable Unicode, control
keys, navigation keys, F1-F35, keypad variants, plain Ctrl+printable C0/DEL controls, and
modifier-wrapped combinations (Shift/Ctrl/Alt/Meta/Super/Hyper) to X11 keysyms. Paste events are
forwarded as literal character key events; the
client enables local bracketed-paste capture, but deliberately does not inject bracketed-paste
delimiters because guacamole-cli cannot yet know whether the remote application enabled bracketed
paste mode. Press `Ctrl-]` to exit the local client loop (`Ctrl-5` is accepted as the same PTY
escape on terminals that report Ctrl-] that way).

This is still a raw passthrough. Full resize handling, clipboard stream integration, fallback UX,
and broader live e2e coverage are tracked as follow-up issues.

## License

Apache-2.0. See [LICENSE](./LICENSE).
