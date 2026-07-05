# Test fixtures

Recorded Guacamole protocol data used across the workspace: by `gua-proto`
parser/round-trip tests (#10), the tunnel handshake golden tests (#19), GUI
snapshot tests (#37), and recording playback (#42).

## Recorded-instruction format (`*.guac`)

A `*.guac` fixture is the **raw Guacamole instruction stream** exactly as it
travels over the tunnel — no framing, no wrapping:

```
LENGTH.VALUE,LENGTH.VALUE,...;LENGTH.VALUE,...;
```

- Each **element** is `<decimal length>.<value>`, where `LENGTH` is the value
  length in **Unicode codepoints, not bytes** (a multi-byte UTF-8 character
  counts as one).
- Elements within an instruction are separated by `,`.
- Each instruction is terminated by `;`.
- The first element of an instruction is its **opcode** (e.g. `select`, `args`,
  `ready`, `blob`).
- Instructions are concatenated with no separator or newline between them. For
  human readability a fixture MAY contain newlines *between* instructions (after
  a `;`); loaders MUST tolerate and strip inter-instruction whitespace, but MUST
  NOT insert whitespace inside an instruction.

### Directionality

Some fixtures capture a full exchange. When direction matters, keep the two
directions in sibling files named `*.c2s.guac` (client→server) and
`*.s2c.guac` (server→client), or annotate in an adjacent `*.md`. A bare
`*.guac` with no suffix is direction-agnostic sample data.

### Provenance

Every non-synthetic fixture SHOULD have a sibling `*.md` (or a top note here)
recording: the `guacamole-client` / `guacd` versions it was captured against,
the protocol (ssh/rdp/vnc/...), and whether any secrets were scrubbed. Capture
tooling arrives with `gua dev record` (#46).

## Files

- `handshake/minimal-ssh.guac` — a synthetic, illustrative SSH connection
  handshake (`select` → `args` → client params → `connect` → `ready`). Values
  are placeholders, safe to commit, and exist so parser tests have a realistic
  shape before live capture (#19/#46) lands.
