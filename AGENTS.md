# AGENTS.md

## Fast Path

- This is one Rust 2024 crate. `src/lib.rs` exports the reusable client/protocol code; Cargo defines `koc_batch`, `koc_cli`, and `token_gen` explicitly.
- Run unit tests with `cargo test`; target one by name with `cargo test <test-substring>`. The current tests cover BON/crypto, Hortor crypto, and study helpers only, not live game traffic.
- Use `cargo check --all-targets` for a fast full compile, and `cargo build --release --bins --examples` for the documented release build.
- For a focused live check, run `cargo run --bin koc_cli -- verify --bin bins/<account>.bin`, then use `cargo run --bin koc_cli -- <subcommand> --help` for the target task's arguments.

## Live-Account Safety

- `bins/*.bin` files are tracked authentication inputs used to request tokens and connect to the live game. Do not alter them or expose their decoded/token contents.
- Do not start `koc_batch` for a narrow test: its first scheduler round force-runs all configured roles and writes `state.json`. Later rounds hot-reload `config.yaml` and changed bin files.
- `koc_cli daily` creates a fresh in-memory `RoleDailyState`; it does not consult the daemon's persisted `state.json` to suppress already-run local tasks. `--group` and `--force-all` are multi-role live actions.
- The examples hard-code a root-level `liulian.bin`, while tracked bins live under `bins/`; `server_list` and `full_flow` make live requests, and `full_flow` runs daily tasks. Prefer `koc_cli` for manual verification.
- `token_gen scan --add-to-config` does not modify `config.yaml`; auto-updating is deliberately disabled to preserve comments, so add the generated bin/role entry manually.

## Architecture

- Add wrapped game commands to `GameClient` in `src/kpi.rs`; it owns login, request timeouts, battle-version handling, and `cmd`/`cmd_fire` helpers. Wire manual commands through `src/cli_*.rs` and `src/cli_command.rs` before scheduling them.
- The wire stack is `websocket.rs` (sequence/response matching and heartbeat) -> `protocol.rs` (BON envelope) -> `bon.rs` and `crypto.rs` (XOR/LZ4). Keep protocol fixes in the layer that owns them.
- The scheduler in `src/scheduler.rs` resolves configured bins/roles, runs work under a `Semaphore`, and persists per-role daily/periodic/weekly state in `state.json`. `config.yaml` role formation and group settings are resolved in `src/config.rs`.
- Treat `error_codes::is_done_error` / `is_done_result` as task-state policy, not just display text: listed server errors are recorded as completed and are not retried.
- `serverId` encodes character index: `base_internal_id + index * 1_000_000` (indices 0-2); the displayed server number is `base_internal_id - 27`. Use `koc_cli verify` to obtain valid IDs for a bin.
