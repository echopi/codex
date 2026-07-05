# cdx Patch Inventory

Intent inventory for every custom patch this fork carries on top of
upstream `openai/codex`. This file is the source of truth for
reapply-from-intent: if a rebase conflict becomes unmanageable, any patch
can be re-implemented on a fresh upstream tag from its entry here.

Maintenance model (tiered by patch depth):

| Tier | Patches | Sync strategy |
|------|---------|---------------|
| Regenerate | T1 | Idempotent transform re-run per tag (script + survivor manifest, fail-closed on unclassified `codex` strings) — planned, currently carried as patch |
| Rebase-first | T2, T4 (infra) · T3, T7 (contained TUI) | Nightly rebase; on conflict, AI reapply from this inventory. Workflows additionally gated by actionlint + dry-run before promotion (planned) |
| Patch-series | T5, T6 | Rebase-carried with mandatory test matrix; reapply is emergency-only. Long-term: upstream as PRs (T5 first) |

Planned promotion gate (not yet implemented): nightly sync lands on
`custom-next`, verification runs, `custom` is promoted only on green and
every sync emits a receipt (base tag, SHAs, steps, verification, decision).

---

## T1: Binary rename codex → cdx

- **Goal**: `cdx` binary coexists with an official `codex` install; shared
  `~/.codex/` config dir (intentional, do not rename).
- **Scope**: `[[bin]]` name, clap `bin_name`/`override_usage`/`after_help`,
  shell completion name, doctor which/where lookups, managed-install file
  names, display labels, resume command strings, test `cargo_bin()`/argv[0],
  Bazel target name, checked-in environment run commands.
- **Acceptance**:
  - `cargo build --release --bin cdx` succeeds; no target named `codex`.
  - `cdx --help` and completion output contain no `codex` invocations.
  - Negative: crate/package names (`codex-*`), config dir `~/.codex/`,
    protocol identifiers, and upstream URLs must NOT be renamed.
- **Tests**: `cargo test -p codex-cli --bin cdx plugin_marketplace`
  (help-usage assertions).
- **Known survivors**: `codex-stdio-to-uds` usage string (separate binary),
  `codex app-server` protocol names, all crate names.

## T2: Daily upstream sync workflow

- **Goal**: `.github/workflows/sync-upstream.yml` — daily UTC 02:00, fetch
  latest stable `rust-v*` tag (alpha excluded), rebase `custom` onto it,
  `--force-with-lease` push, fail loudly on conflict.
- **Acceptance**: on a no-new-tag day the run is a no-op exiting 0;
  on conflict it must NOT push anything.
- **Negative cases to preserve on reapply**: never push after conflict,
  empty tag resolution, or missing `rust-v*` match.
- **Tests**: none automated yet (planned: dry-run mode against synthetic tags).

## T3: /btw split from /side (lightweight ephemeral question)

- **Goal**: `/btw` = single-turn, no-tools, ephemeral quick question that
  does not enter main-thread history; `/side` keeps upstream multi-turn
  fork behavior unchanged.
- **Mechanism**: `btw_mode` flag through `AppEvent::StartSide` →
  `SideThreadState`; `btw_fork_config()` appends no-tools developer
  instructions; distinct slash-command description and UI labels.
- **Acceptance**:
  - `/btw q` forks with btw_mode, `/side q` without (dispatch tests).
  - Negative: `/side` label/flow byte-identical to upstream behavior.
- **Tests**: `cargo test --release -p codex-tui btw` (7),
  `cargo test --release -p codex-tui side` (75),
  slash-popup insta snapshot (`slash_popup_bt`).

## T4: Multi-platform release workflow

- **Goal**: `.github/workflows/release.yml` — on `cdx-v*` tag: build
  x86_64-linux (ubuntu), x86_64-mac (cross-compiled on macos-14 arm —
  macos-13 Intel runners are retired), aarch64-mac; publish GitHub Release.
- **Hard-won constraints (preserve on reapply)**:
  - `rustup target add` must run inside `codex-rs/` so the
    rust-toolchain.toml-pinned toolchain gets the cross target (dtolnay
    action only installs targets for stable) — else E0463.
  - `fail-fast: false` so one platform leg cannot cancel the others.
- **Acceptance**: all three assets on the Release page; binary names
  `cdx-<target>`.

## T5: MCP channel active-session ingress

- **Goal**: opt-in per-server `surface_notifications` lets trusted MCP
  servers push channel messages (e.g. DingTalk) into the active session as
  model-visible input; reply flows through existing MCP tools. Spec:
  `docs/` (pod: codex-active-session-ingress-spec.md).
- **Mechanism**: ChannelNotification envelope parser
  (`notifications/channel`, `notifications/claude/channel`,
  `notifications/message` + `toSession:true`) → DedupeStore (TTL 1h, cap
  1024) → OnChannelNotification callback (Weak<Session>) → InputQueue
  `channel_pending` (cap 64, drop-oldest) → idle-wake or busy-queue drain
  after mailbox items.
- **Security invariants (MUST hold under reapply — test each)**:
  1. Default-off: without `surface_notifications = true` a channel
     notification is logged and dropped on every path.
  2. `notifications/message` without `toSession: true` is never surfaced.
  3. Duplicate `(server, source, msgId)` within TTL produces exactly one
     model-visible input.
  4. Size cap covers content PLUS all metadata string fields (16 KiB).
  5. Message body cannot break out of the `<channel>` XML wrapper
     (content is XML-escaped; wrapper tag count stays 1/1).
  6. External input grants no approval/sandbox/permission bypass; it is
     plain user-visible text.
  7. Queue and dedupe stores are bounded (no unbounded memory from a
     flooding server).
  8. Work enqueued during turn teardown is picked up by the post-teardown
     recheck in `on_task_finished` (no wake-miss).
- **Tests**: `cargo test --release -p codex-protocol channel` ·
  `-p codex-rmcp-client channel` · `-p codex-config mcp_types` ·
  `-p codex-core --lib input_queue` (REPOSITORY_NAME=_main required on
  pod builds).

## T6: Backtrack workspace restore (planned)

- Spec: codex-workspace-restore-spec.md (pod). Per-turn ephemeral git
  snapshot ref before mutating turns; Esc-Esc backtrack gains opt-in
  "also restore workspace"; conversation-only remains the default.

## T7a: /btw completion hold

- **Goal**: on TurnCompleted the btw thread is NOT auto-discarded (the
  auto-return raced the user's reading); label flips to
  `[btw] ✓ done · Ctrl+C to return`; existing Ctrl+C return path discards
  the thread, preserving ephemerality.
- **Acceptance**:
  - Completion marks state exactly once; follow-up turn start resets the
    flag so the next completion re-marks (no latch).
  - Negative: non-btw side threads are never marked; discard-on-return
    behavior unchanged.
- **Tests**: `cargo test --release -p codex-tui btw_turn_completion`.

## T7b: /btw inline answer cell (planned)

- Render the btw answer as a dimmed, render-only cell in the main view
  (no screen switch, not persisted to rollout); includes evaluating
  SessionStart-hook skip for btw forks.
