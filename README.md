# KOC Core

[中文版](README-zh.md)

A Rust implementation of the KOC game automation core library. It includes token management, WebSocket communication, daily-task scheduling, WeChat QR-code login, and more.

## Project Structure

```
koc_core/
├── src/
│   ├── lib.rs              # Library entry point; KocCore core struct
│   ├── bon.rs              # BON (Binary Object Notation) encoding/decoding
│   ├── crypto.rs           # Encryption/decryption (XOR / LZ4)
│   ├── protocol.rs         # Game message protocol (ProtoMsg parsing/creation)
│   ├── http_client.rs      # HTTP client (authuser / serverlist)
│   ├── websocket.rs        # WebSocket client (heartbeat, request/response matching)
│   ├── proxy.rs            # Realtime byte-preserving WebSocket relay
│   ├── proxy_capture.rs    # Capture schema, decode, correlation, command catalog
│   ├── kpi.rs              # High-level API (login, 155 game commands, daily/periodic tasks)
│   ├── error_codes.rs      # Game error-code map + is_done_error decisions
│   ├── config.rs           # Configuration parsing (YAML) + ConfigWatcher hot reload
│   ├── state.rs            # Runtime state persistence (JSON) + AppState/RoleState
│   ├── scheduler.rs        # Batch scheduler (concurrency control, round management, hot reload)
│   ├── hortor_crypto.rs    # Hortor platform payload encryption (cipher table + XOR)
│   ├── wx_login.rs         # WeChat QR-code login + Hortor login + bin generation
│   ├── logging.rs          # tracing log initialization (console + file)
│   ├── koc_batch.rs        # [bin] Batch daily-task scheduler CLI
│   ├── token_gen.rs        # [bin] Token/bin-file generator CLI
│   ├── koc_cli.rs          # [bin] Manual task verification CLI (verify/study/tower/evotower/monthly/car/skinc)
│   └── koc_proxy.rs        # [bin] Realtime protocol relay and capture inspector
├── run_koc_tasks.ps1       # Combined interactive task runner
├── run_koc_cli.ps1         # New two-level interactive menu runner
├── examples/
│   ├── parse_bin.rs        # Parse bin-file contents
│   ├── server_list.rs      # Fetch character list (with debugging)
│   ├── server_list_json.rs # Export the full serverlist as JSON
│   └── full_flow.rs        # Complete single-character flow demonstration
├── config.yaml             # Scheduler configuration example
├── Cargo.toml
├── README.md                # English documentation
└── README-zh.md             # Chinese documentation
```

## Build

```bash
# Build everything
cargo build --release --bins --examples

# Build only the scheduler
cargo build --release --bin koc_batch

# Build only the token generator
cargo build --release --bin token_gen

# Build only the manual verification tool
cargo build --release --bin koc_cli

# Build only the realtime protocol proxy
cargo build --release --bin koc_proxy
```

## Tool 1: token_gen - Token/Bin File Generation

A standalone token/bin-file generator. It currently supports WeChat QR-code login and can be extended with more methods later.

### Generate a bin with WeChat QR-Code Login

```bash
# Basic usage: after scanning, the bin is saved to bin_output_dir configured in config.yaml
./token_gen scan

# Specify an output directory
./token_gen scan -o /data/bins/

# Print a YAML snippet that can be appended to config.yaml after generation
./token_gen scan --add-to-config

# Specify a configuration file
./token_gen -c /path/to/config.yaml scan
```

**Flow:**

```
The terminal displays an ASCII QR code → confirm the scan in WeChat → extract the OAuth code
→ log in to the Hortor platform (encrypted payload) → obtain user credentials
→ construct bin data (BON encoding + encryption) → save to a file
```

**Example output:**

```
=== KOC Token Generator ===
[token_gen] Output dir: bins/

[token_gen] Fetching WeChat QR code...
[token_gen] Please scan with WeChat (120s timeout):

  █▀▀▀▀▀█ ▄█▀▄ █▀▀▀▀▀█
  █ ███ █ ▀▄▀▄ █ ███ █
  ...

[token_gen] Got OAuth code: 091y...IDG2 (len=32)
[token_gen] Scan confirmed! User: phoenix
[token_gen] Hortor login successful
[token_gen] Bin saved: bins/phoenix.bin (1061 bytes)
[token_gen] Done!
```

### CLI Arguments

```
token_gen [OPTIONS] <COMMAND>

Commands:
  scan    Generate a bin file with WeChat QR-code login

Options:
  -c, --config <CONFIG>    Configuration file path [default: config.yaml]
  -h, --help               Show help

scan subcommand:
  -o, --output <DIR>       Output directory (overrides bin_output_dir in config)
      --add-to-config      Print a YAML snippet that can be appended to config.yaml
```

## Tool 2: koc_cli - Manual Task Verification CLI

For manually verifying a single bin / single-character flow. Once a feature is stable, it can be moved into batch scheduling.

- The `verify` subcommand quickly validates a bin and lists its characters.
- The `study` subcommand reuses the core modules (`KocCore` + `GameClient` + `study::run_study`) to perform the complete quiz flow.
- The `tower` subcommand reuses the core modules (`fight_starttower/tower_getinfo/tower_claimreward`) to climb the tower automatically.
- The `evotower` subcommand reuses the core modules (`evotower_getinfo/evotower_readyfight/evotower_fight`) to climb Evo Tower automatically.
  - Order: claim the free item (`mergebox`) first → climb Evo Tower → run one merge-synthesis pass last.
- The `skinc` subcommand manually runs the skin-change challenge. It validates the availability period, the active Boss, and filters cleared floors automatically.
- The `monthly` subcommand queries monthly-task progress and tops up normal fishing-rod/fighting-arena tasks when needed.
- The `car` subcommand manually dispatches or claims vehicles, using intelligent dispatch logic (including refresh-coupon evaluation and guard assignment).
- The `info` subcommand queries full JSON for role info or Evo Tower info.
- The `daily` subcommand manually runs daily tasks, supporting single-character, group, and all-character modes.
- The `group` subcommand shows the character groups defined in config.yaml.

```bash
# Verify a bin and list its characters
./koc_cli verify --bin bins/phoenix.bin

# Verify and optionally check whether a specified serverId exists
./koc_cli verify --bin bins/phoenix.bin --server-id 12489

# Run the quiz task (specified serverId)
./koc_cli study --bin bins/phoenix.bin --server-id 13007

# Force the quiz (ignore this week's completed state)
./koc_cli study --bin bins/phoenix.bin --server-id 13007 --force

# Climb the tower automatically (until out of energy, cleared, or maximum attempts reached)
./koc_cli tower --bin bins/phoenix.bin --server-id 13007

# Climb the tower automatically (custom parameters)
./koc_cli tower --bin bins/phoenix.bin --server-id 13007 --max-climb 60 --interval-ms 1200 --refresh-every 5

# Climb Evo Tower automatically (by default, stop only after 3 consecutive request failures)
./koc_cli evotower --bin bins/phoenix.bin --server-id 13007

# Climb Evo Tower automatically (custom parameters)
./koc_cli evotower --bin bins/phoenix.bin --server-id 13007 --max-climb 120 --interval-ms 1000 --failure-limit 3

# Climb Evo Tower automatically and adjust merge-loop parameters
./koc_cli evotower --bin bins/phoenix.bin --server-id 13007 --merge-max-loops 30 --merge-delay-ms 600

# Run the skin-change challenge automatically (one character)
./koc_cli skinc --bin bins/phoenix.bin --server-id 13007

# Run the skin-change challenge automatically (by group)
./koc_cli skinc --group Primary

# Query monthly-task progress (default behavior)
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007

# Query monthly fishing progress only
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007 --mode fish

# Top up fighting-arena monthly tasks to the progress due for the current date
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007 --topup --mode arena

# Fill fishing-rod and fighting-arena monthly tasks directly to their limits
./koc_cli monthly --bin bins/phoenix.bin --server-id 13007 --topup --mode all --complete

# Intelligent vehicle dispatch (available before 20:00 on Monday/Tuesday/Wednesday; includes refresh-coupon/grand-prize evaluation)
./koc_cli car --bin bins/phoenix.bin --server-id 13007

# Claim a vehicle only (do not dispatch)
./koc_cli car --bin bins/phoenix.bin --server-id 13007 --action claim

# View role info (default)
./koc_cli info --bin bins/phoenix.bin --server-id 13007

# View Evo Tower info
./koc_cli info --bin bins/phoenix.bin --server-id 13007 --type evotower

# Run daily tasks (one character)
./koc_cli daily --bin bins/phoenix.bin --server-id 13007

# Run daily tasks (by group; do not dispatch vehicles or use gacha)
./koc_cli daily --group Primary --skip-car --skip-gacha

# Run daily tasks (all characters)
./koc_cli daily --force-all

# View character groups
./koc_cli group
```

### CLI Arguments

```text
koc_cli [OPTIONS] <COMMAND>

Commands:
  verify   Verify a bin and list characters
  study    Run the weekly quiz task
  monthly  Query or top up monthly tasks
  tower    Automatic tower-climbing task
  evotower Automatic Evo Tower-climbing task
  skinc    Automatic skin-change challenge task
  car      Intelligent vehicle dispatch/claim
  info     Query role info or Evo Tower info
  daily    Run daily tasks (single character/group/all)
  gacha    Free gacha draw
  group    View character groups

Options:
  -c, --config <CONFIG>        YAML configuration file path [default: config.yaml]

verify subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Optional; check whether the target character exists

study subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)
  --force                     Force the quiz; ignore this week's completed state

monthly subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)
  --mode <MODE>               fish|arena|all (default: all)
  --topup                     Explicitly run the top-up; query only by default
  --complete                  Use with --topup to fill directly to the monthly limit
  --arena-safety-max <N>      Safety limit for fighting-arena top-ups (default: 100)
  --fish-batch-size <N>       Paid-fishing attempts per batch (default: 10)
  --no-claim-fish-point       Disable automatic claiming of accumulated fishing-rod rewards

tower subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)
  --max-climb <N>             Maximum tower-climbing attempts (default: 100)
  --interval-ms <MS>          Milliseconds between tower climbs (default: 1000)
  --refresh-every <N>         Force-refresh role info every N attempts (default: 5)
  --no-auto-claim             Disable automatic claiming of Salted General Tower rewards

evotower subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)
  --max-climb <N>             Maximum tower-climbing attempts (default: 100)
  --interval-ms <MS>          Milliseconds between tower climbs (default: 1000)
  --refresh-every <N>         Refresh Evo Tower information every N attempts (default: 3)
  --failure-limit <N>         Stop threshold for consecutive request failures (default: 3)
  --no-auto-claim-task        Disable automatic claiming of Evo Tower daily-task rewards
  --no-auto-claim-reward      Disable automatic claiming of Evo Tower chapter rewards
  --merge-max-loops <N>       Maximum merge-synthesis loop count (default: 20)
   --merge-delay-ms <MS>       Milliseconds between merge-flow loops (default: 500)

skinc subcommand:
  --bin <BIN>                 Single-character mode: bin file path
  --server-id <SERVER_ID>     Single-character mode: target serverId
  --group <NAME>              Group mode: run every character in this config.yaml group
  --force-all                 All-character mode: run every character in config.yaml

car subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)
  --action <ACTION>           Action: send (dispatch + claim) or claim (claim only) [default: send]

info subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)
  --type <TYPE>               Information type: role (default) or evotower

daily subcommand:
  --bin <BIN>                 Single-character mode: bin file path
  --server-id <SERVER_ID>     Single-character mode: target serverId
  --group <NAME>              Group mode: run every character in the group defined in config.yaml groups
  --force-all                 All-character mode: run every character in config.yaml
  --skip-car                  Skip vehicle dispatch/claim
  --skip-gacha                Skip free gacha

gacha subcommand:
  --bin <BIN>                 Bin file path
  --server-id <SERVER_ID>     Target character serverId (required)

group subcommand:
  (no arguments; list groups defined in groups and their member characters)
```

Evo Tower claim details:
- Each `evotower_claimtask` run first restores the day's claimed-task state from the server's `taskClaimMap`.
- `evotower_claimtask` is triggered as tower climbs increase: on the 3rd attempt use `task_id=1`, on the 6th use `task_id=2`, and on the 10th use `task_id=3` (3/3/4 intervals).
- After a `task_id` is claimed successfully, it is not claimed again during the same run.
- Error code `12200050` is treated as already claimed and is not requested again.
- Error code `12200040` is treated as unmet conditions and can be tried again later after conditions are met.
- If `evotower_readyfight` returns `12200020`, the current chapter reward is claimed first and the call is retried.

Skin-change challenge details:
- Automatically validate that the account is within the skin-change challenge's 7-day availability period (by parsing and checking `actId`).
- Select the Boss available that day from the actual system weekday (Boss1 on Friday through Boss6 on Wednesday; Boss1 through Boss6 are all available on Thursday).
- Automatically exclude Bosses already cleared through floor 8 according to the `levelRewardMap` returned by the server.
- Retry failed battles automatically. A Boss is skipped after 3 consecutive failed attempts to prevent an infinite loop.

Monthly-task details:
- The default behavior is to query; it does not top up anything.
- Query output includes the current normal fishing-rod balance and fighting-arena ticket balance.
- A top-up runs only with `--topup`.
- By default, tasks are topped up to the progress due for the current date; `--complete` fills directly to the monthly limit.
- Nothing is topped up when the current progress meets or exceeds the target.
- `fish` does not depend on YAML formation configuration.
- `arena`/`all` read the fighting-arena formation from YAML and restore the original formation after the task.

Vehicle details:
- Available only before 20:00 on Monday/Tuesday/Wednesday (the dispatch window).
- `send` claims first and then dispatches; `claim` only claims.
- Dispatch strategy: on Monday/Tuesday, dispatch when a refresh coupon is available and use one while waiting for a refresh coupon; on Wednesday, refresh for grand prizes.
- Vehicles of red quality or higher automatically receive Legion guards (each guard is limited to 4 assignments).
- After claiming, the engine is upgraded and part-consumption rewards are claimed automatically.

Info details:
- After login, query the complete server data for the specified role and print formatted JSON to stdout.
- `--type role` prints the `role_getroleinfo` result (including hangup, salt jar, tower, statistics, and more).
- `--type evotower` prints the `evotower_getinfo` result (including energy, tower floor, task state, and more).

Daily-task details:
- Three modes: single character (`--bin`/`--server-id`), group (`--group`), and all characters (`--force-all`).
- Reuses the scheduler's core logic and reads the formations/dream_shop configuration in config.yaml.
- Tasks already completed on the server are skipped automatically.
- Does not read or write state.json; every execution is an independent session.

Gacha details:
- Calls `gacha_drawreward` to perform one free gacha draw.
- Uses server-side `statisticsTime["gacha:free"]` to determine whether today's draw was already made.
- If it was already drawn, skip without calling the API again.

Group details:
- Lists every group declared under `groups` in config.yaml and the characters each contains.
- Lists all characters when there are no groups.
- Suitable for redirecting to a file: `./koc_cli info ... > role.json`

## Helper Interactive Scripts (Scripts)

On Windows, if you do not want to enter long command-line arguments manually every time, run the PowerShell scripts in the repository root for interactive batch operations:

- `run_koc_tasks.ps1`: Combined interactive entry point. Use its terminal menu to select `tower`, `evotower`, `car`, or run all tasks at once, then batch-run them with predefined ID groups.
- `run_koc_cli.ps1`: A new two-level state-machine menu script. The first level selects a target subcommand; the second dynamically parses `config.yaml` to generate a structured `Bin + ServerId` list. It supports running one account or a whole group precisely, retains logs after execution, and returns seamlessly.

## Tool 3: koc_batch - Batch Daily-Task Scheduler

A resident background scheduler that manages daily and periodic tasks for every character across multiple bin files.

### Start

```bash
# Run in the foreground
./koc_batch -c config.yaml -s state.json

# Run in the background
nohup ./koc_batch -c config.yaml -s state.json > koc.log 2>&1 &

# Ctrl+C exits gracefully (waits for the current task to finish)
```

### CLI Arguments

```
koc_batch [OPTIONS]

Options:
  -c, --config <CONFIG>    Configuration file path [default: config.yaml]
  -s, --state <STATE>      State file path [default: state.json]
  -h, --help               Show help
```

### Scheduling Strategy

After startup, the program enters its main loop with three kinds of tasks:

**A. Daily tasks** (triggered once per day at `schedule_time`)

| Task | Description |
|------|------|
| Share game | Extend hangup time |
| Send gold to friends | Send in batches |
| Free recruitment | Once daily |
| Intelligent hangup | If remaining time is <=1h, claim + extend; if the cap is <=8h, extend; otherwise save time |
| Free gold purchase | 3 times daily |
| Open chests | Wooden chests x10 |
| Black Market purchases | Once daily |
| Salt jar | Intelligent handling: claim if stopped; stop & start to extend time |
| Fighting arena | Enter → get opponents → fight x3 (level >=400) |
| Boss battle | 3 times daily (select Boss by weekday) |
| Club Boss | 2 free times daily (intelligently checks remaining attempts) |
| Welfare check-in | Once daily |
| Club check-in | Once daily |
| Discount gift pack | Once daily |
| Card gift pack | Free card + permanent card |
| Treasure Pavilion | Daily free reward |
| Free fishing | 3 times daily |
| Genie sweep | One each for Wei/Shu/Wu/Qun + free sweep tickets x3 (level >=3000) |
| Free gacha | Once daily |
| Legacy fragments | Claim hangup rewards (level >8000) |
| Task points | Claim daily/weekly task rewards |
| Mail | Claim all attachments at once |
| Club vehicle dispatch | Intelligent dispatch on Monday/Tuesday/Wednesday, with refresh-coupon/grand-prize evaluation |
| Club vehicle claim | Claim automatically 4 hours after dispatch + upgrade engine |

**B. Periodic tasks** (triggered by time thresholds and checked every round)

| Task | Trigger condition | Default threshold |
|------|----------|----------|
| Hangup extension + claim | Claim + extend 4 times when remaining <=1h or expired; only extend when the cap <=8h | Dynamic evaluation |
| Salt-jar extension | Remaining time <= threshold | 1 hour |
| Claim legacy fragments | Time since last claim >= interval | 4 hours |
| Club vehicle claim | Claim again >=4h after the first daily vehicle claim | Dynamic evaluation |
| Salted General Tower | Climb after energy refills to 10; loop `fight_starttower` | Dynamic evaluation |
| Evo Tower | Climb after Black Market week energy refills to 10, including daily task/chapter reward/mergebox (level >=7000) | Dynamic evaluation |

**C. Weekly tasks** (run once on `weekly_schedule_day`)

| Task | Description |
|------|------|
| Quiz | Answer automatically (10 questions); mark this week as done when complete |

### Intelligent Features

- **State persistence**: Each daily-task subitem is independently marked complete and saved to `state.json`, so it is not repeated after restart.
- **Intelligent skip**: Tasks already completed on the server are skipped automatically; "completed" error codes are marked done and are not retried.
- **Configuration hot reload**: Each round checks the modification times of `config.yaml` and bin files, reloading them automatically when changed.
- **State synchronization**: When bins/roles are added or removed in configuration, state.json synchronizes automatically (add empty state, remove orphans).
- **Concurrency control**: A Semaphore controls the maximum number of concurrent WebSocket connections.
- **Structured logging**: Based on `tracing`; concurrent contexts automatically include `round/role`.
- **Dual output**: Output to both the console and daily-rotated files in `logs/`.

### Example Log Output

```
2026-04-16 13:13:51  INFO RT{R=5 role=erqiao/15001-0}: task: [OK] Daily Boss #3/3 (id=9903)
2026-04-16 13:13:51  INFO RT{R=5 role=erqiao/12980-0}: task: [~~] Free gift card code=-10006 error=[-10006] Today's reward already claimed or attempts exhausted
2026-04-16 13:13:52  WARN RT{R=5 role=erqiao/15001-0}: task: [X] Claim legacy reward code=800040 error=[800040] unknown code
```

Log markers:
- `[OK]` Executed successfully
- `[~~]` Already complete; skipped
- `[X]` Execution failed (with error code and English description)

Log time and format:
- Time is `UTC+8`, in `YYYY-MM-DD HH:MM:SS` format.
- The console has no color by default (convenient for redirection and grep).

Log level and output:
- The default level is `info`; adjust it with an environment variable: `RUST_LOG=debug ./koc_batch`
- File log directory: `logs/` (rotated daily)

## Tool 4: koc_proxy - Realtime Protocol Inspection

`koc_proxy` relays complete WebSocket messages without modifying their payloads and decodes a copy of each Binary message through the existing XOR/LZ4/BON stack. Decode failures never modify or block the forwarded message. The relay listens on loopback only and does not capture the authentication query from the WebSocket handshake.

Start a realtime relay with optional raw recording and command-catalog output:

```bash
cargo run --bin koc_proxy -- relay \
  --listen 127.0.0.1:8787 \
  --decode \
  --record captures/session.jsonl \
  --catalog captures/catalog.json
```

Point this project's WebSocket client at the relay in another terminal. The token remains in the normal login flow and is not passed on the command line:

```bash
KOC_WS_BASE_URL=ws://127.0.0.1:8787 \
  cargo run --bin koc_cli -- info --bin bins/<account>.bin --server-id <ID>
```

Without `KOC_WS_BASE_URL`, `koc_cli` connects directly to the upstream server and no relay records are produced. Session JSONL and catalog snapshots are flushed while the relay is running, so they can be inspected before shutdown.

For a persistent shell setting, export the variable and verify that child processes can see it. `echo $KOC_WS_BASE_URL` alone does not prove it was exported:

```bash
export KOC_WS_BASE_URL=ws://127.0.0.1:8787
env | grep '^KOC_WS_BASE_URL='
```

Realtime output is enabled by default. It supports JSON output and command/direction filtering:

```bash
cargo run --bin koc_proxy -- relay --format json --cmd 'activity_*'
cargo run --bin koc_proxy -- relay --direction server-to-client
```

Saved records can be decoded again after the protocol parser improves:

```bash
cargo run --bin koc_proxy -- decode --input captures/session.jsonl
cargo run --bin koc_proxy -- catalog \
  --input captures/session.jsonl \
  --output captures/catalog.json
```

`--record` stores raw payloads as JSONL and therefore does not contain plaintext `cmd` fields. `--decode` prints decoded events to the relay console. To inspect the command timeline from a saved session, run `koc_proxy decode`; to list command names only, query the catalog JSON:

```bash
./target/release/koc_proxy decode \
  --input capture/session.jsonl \
  --format json \
  --cmd '*tower*'

jq -r '.commands | keys[]' capture/catalog.jsonl
```

An external TLS MITM can stream already reassembled WebSocket messages as capture-record JSONL. `koc_proxy` does not terminate arbitrary TLS connections itself:

```bash
external-adapter | cargo run --bin koc_proxy -- inspect --stream
```

Capture files are created with mode `0600` on Unix and `captures/` is ignored by Git. Decoded output redacts common token/session identifiers unless `--show-sensitive` is explicitly supplied. Raw payloads can still contain private role data and must not be committed.

Existing regular capture and catalog files are overwritten when a relay session starts. Symlinks and non-file paths are rejected. Remote upstreams must use `wss://`; plaintext `ws://` is accepted only for loopback development servers.

## Configuration File: config.yaml

```yaml
# Maximum number of concurrent WebSocket connections
concurrency: 5

# Delay between starting each role (ms; prevents server rate limiting)
delay_between_ms: 2000

# Daily-task execution time (24-hour HH:MM format)
schedule_time: "06:00"

# Friday override for daily-task start time (24-hour HH:MM format)
friday_daily_start_time: "12:10"

# Maximum number of daily-task retries
max_daily_retries: 1

# Club vehicle-dispatch feature (Monday/Tuesday/Wednesday, schedule_time~20:00)
car_enabled: true

# Day to execute weekly tasks (Mon/Tue/Wed/Thu/Fri/Sat/Sun)
weekly_schedule_day: "Sat"

# Quiz task (default true)
study_enabled: true

# Salted General Tower (default true; climb after energy refills to 10, 24/7)
tower_enabled: true

# Evo Tower (default true; climb only after energy refills to 10 during Black Market week)
evotower_enabled: true

# Free gacha (default true)
gacha_enabled: true

# Main-loop check interval (seconds)
check_interval_secs: 1800

# Hangup: claim + extend after this many hours of hangup time
hangup_threshold_hours: 8.0

# Salt jar: stop & start to extend time when remaining time is below this many hours
bottle_threshold_hours: 1.0

# Legacy fragments: claim once every this many hours
legacy_interval_hours: 4.0

# Bin-file output directory (where token_gen QR-code generated bins are stored)
bin_output_dir: bins/

# Default bin directory used by batch/profile
default_bin_path: bins

formations:
  defaults:
    arena: 1
    tower: 1
    evotower: 1
    boss_daily: 1
    boss_legion: 1

# Optional: override specific formations for one role
# bins:
#   - bin: account.bin
#     roles:
#       - server_id: 10000
#         formations:
#           tower: 2
#           evotower: 3
#           boss_daily: 4
#           boss_legion: 4
#         dream_shop:
#           enabled: true
#           purchase_list:
#             - "1-5" # Basic merchant - Salted-Fish God ticket
#             - "1-6" # Basic merchant - Salted-Fish God torch
#             - "3-2" # Advanced merchant - golden fishing rod

dream_shop_presets:
  basic_daily_shop:
    enabled: true
    purchase_list:
      - "1-5" # Basic merchant - Salted-Fish God ticket
      - "1-6" # Basic merchant - Salted-Fish God torch
      - "3-1" # Advanced merchant - platinum chest
      - "3-2" # Advanced merchant - golden fishing rod

# Maintenance-window configuration: skip the whole round when matched (do not attempt to connect to the server)
maintenance_windows:
  - weekday: Fri
    start_time: "05:00"
    end_time: "07:00"
  - weekday: Sat
    start_time: "19:15"
    end_time: "21:15"
  - weekday: Sun
    start_time: "19:15"
    end_time: "20:45"

# === Character groups (used by CLI --group) ===
groups: []
#   - Primary
#   - Secondaly

# === Bin-file list ===
bins:
  - bin: account.bin
    roles:
      - server_id: 10000  # Roles must be listed explicitly
        # group: Primary         # Optional; a group name declared in the groups list
        formations:
          tower: 2
          evotower: 2
      - server_id: 1010000
      - server_id: 2010000
```

### bins Configuration Details

- `bin`: Bin file name (used by batch/profile).
- `roles[].server_id`: List of characters to process; must be listed explicitly.
- `roles[].formations`: Optional; overrides formations only for specific scenarios of this role.
- `roles[].dream_shop`: Optional Dream Shop purchase configuration (disabled by default); supports inline configuration or a `dream_shop_presets` name reference.
- `roles[].group`: Optional character-group label (must be declared in the `groups` list, otherwise it is skipped with a warning).
- `verify/study` do not read a profile; they use only the explicit `--bin` file path.
- `tower/evotower` read formation configuration from YAML, switch formations before the task, and restore the original formation afterward.

### Formation Scenario Keys

- `arena`: Fighting arena
- `tower`: Salted General Tower
- `evotower`: Evo Tower
- `boss_daily`: Daily Boss
- `boss_legion`: Legion Boss

Details:
- When `roles[].formations` is not configured, use `formations.defaults`.
- `koc_batch` currently integrates `arena` / `boss_daily` / `boss_legion`.
- `koc_batch` daily includes Salted-Fish King Dreamscape (Sunday/Monday/Wednesday/Thursday).
- `koc_batch` daily can run Dream Shop purchases configured by role (on the same days that Salted-Fish King Dreamscape is open).
- `koc_cli tower` / `koc_cli evotower` integrate their corresponding scenario formations.
- `verify/study` do not switch formations.

### Dream Shop `purchase_list` Mapping

Format: `merchantId-itemIndex`

Basic merchant (1):
- `1-0`: Advancement stone
- `1-1`: Refined iron
- `1-2`: Wooden chest
- `1-3`: Bronze chest
- `1-4`: Normal fishing rod
- `1-5`: Salted-Fish God ticket
- `1-6`: Salted-Fish God torch

Intermediate merchant (2):
- `2-0`: Nightmare crystal
- `2-1`: Advancement stone
- `2-2`: Refined iron
- `2-3`: Golden chest
- `2-4`: Golden fishing rod
- `2-5`: Recruitment order
- `2-6`: Orange general fragment
- `2-7`: Purple general fragment

Advanced merchant (3):
- `3-0`: Nightmare crystal
- `3-1`: Platinum chest
- `3-2`: Golden fishing rod
- `3-3`: Recruitment order
- `3-4`: Red general fragment
- `3-5`: Orange general fragment
- `3-6`: Red general fragment
- `3-7`: Normal fishing rod

Details:
- `purchase_list` configures product types, not the shop position `pos`.
- At runtime, the current position is resolved from `role.dungeon.merchant`, then purchases are made in ascending `merchantId` order and descending `pos` order.
- Runs only on Sunday/Monday/Wednesday/Thursday, and requires `levelId >= 1000`.

### Time-Window Details

- `schedule_time`: Regular daily-task start time.
- `friday_daily_start_time`: Friday override start time (default `12:10`).
- `maintenance_windows`: Maintenance-window configuration; skip the whole round inside a window and do not attempt to connect to the server.
- Repeated, overlapping, or adjacent windows are deduplicated and merged at startup; invalid windows (invalid time format or `start>=end`) are discarded.

### serverId Encoding Rules

```
serverId = actual internal ID + character index * 1000000
actual displayed server number = internal ID - 27

Examples:
  13007         → server 12980, character 0
  1013007       → server 12980, character 1
  2013007       → server 12980, character 2
```

## Examples

### parse_bin - Parse a Bin File

```bash
# Run from the koc_core/ directory; the bin-file path is specified in code
cargo run --example parse_bin
```

Decrypts and displays fields in a bin file (platform, info, serverId, and more).

### server_list - Fetch Character List

```bash
cargo run --example server_list
```

Sends bin data to the server and retrieves and displays information for every character in the account (name, server, combat power, level).

### server_list_json - Export Complete JSON

```bash
cargo run --example server_list_json
```

Exports the complete serverlist data returned by the server to `server_list.json`, including all server information and character details.

### full_flow - Complete Single-Character Flow

```bash
cargo run --example full_flow
```

Demonstrates the complete automation flow for one character:
1. Fetch character list
2. Select the strongest character
3. Obtain token
4. WebSocket login (including randomSeed synchronization)
5. Run daily tasks (intelligently skip completed tasks)
6. Disconnect

## Game Command API

`GameClient` provides 155 game-command methods, grouped by function:

| Group | Commands | Examples |
|------|--------|------|
| System/login | 7 | `system_signinreward`, `system_buygold` |
| Friends | 1 | `friend_batch` |
| Generals | 8 | `hero_recruit`, `hero_heroupgradelevel` |
| Items/chests | 3 | `item_openbox`, `item_batchclaimboxpointreward` |
| Fighting arena | 3 | `arena_startarea`, `arena_getareatarget` |
| Battle | 7 | `fight_startboss`, `fight_startareaarena` |
| Tasks | 3 | `task_claimdailypoint`, `task_claimdailyreward` |
| Store | 4 | `store_purchase`, `store_refresh` |
| Club/Legion | 20+ | `legion_signin`, `fight_startlegionboss` |
| Mail | 4 | `mail_claimallattachment`, `mail_getlist` |
| Quiz | 3 | `study_startgame`, `study_answer` |
| Artifacts/fishing | 4 | `artifact_lottery`, `artifact_load` |
| Genie | 2 | `genie_sweep`, `genie_buysweep` |
| Salt jar | 3 | `bottlehelper_claim`, `bottlehelper_start` |
| Salted General Tower | 2 | `tower_getinfo`, `fight_starttower` |
| Evo Tower | 6 | `evotower_getinfo`, `evotower_fight` |
| Salted-Fish King Treasury | 4 | `bosstower_getinfo`, `bosstower_startboss` |
| Merge box | 7 | `mergebox_getinfo`, `mergebox_automergeitem` |
| Vehicles | 7 | `car_getrolecar`, `car_send` |
| Legacy | 6 | `legacy_claimhangup`, `legacy_gift_send` |
| Equipment | 3 | `equipment_quench`, `equipment_confirm` |
| Other | 40+ | `presetteam_saveteam`, `rank_getserverrank` ... |

All commands can also be called through the generic interface:

```rust
// Wait for the response
let result = game.cmd("any_command", json!({"key": "value"})).await?;

// Do not wait for the response
game.cmd_fire("any_command", json!({})).await?;
```

## Error Codes

The built-in map contains 30+ English game error-code descriptions, divided into two categories:

**Completed/cannot act** (mark done; do not retry):
- `400190` No check-in rewards available to claim
- `2300190` Already checked in today
- `200160` Feature not unlocked
- `-10006` Today's reward already claimed or attempts exhausted
- ... (25 total)

**Other errors** (do not mark done; retry next round):
- `200400` Action performed too quickly; try again later
- Network timeouts and similar errors

## Technical Architecture

```
┌──────────────────────────────────────────────┐
│              token_gen CLI                    │
│  (WeChat QR code / other bin methods)            │
└──────────────────┬───────────────────────────┘
                   │ Generate .bin files
                   ▼
┌──────────────────────────────────────────────┐
│            koc_batch CLI                     │
│  (Resident scheduler; reads config.yaml)      │
│                                              │
│  ┌─ Scheduler ──────────────────────────┐    │
│  │  Main loop (checks every 60s)          │    │
│  │  ├─ Config hot reload (ConfigWatcher)  │    │
│  │  ├─ State sync (sync_with_roles)       │    │
│  │  └─ Concurrent execution (Semaphore)   │    │
│  └──────────────────────────────────────┘    │
│           │                                  │
│     ┌─────┼─────┐                            │
│     ▼     ▼     ▼                            │
│   tokio  tokio  tokio  (parallel role tasks)  │
│     │     │     │                            │
│     ▼     ▼     ▼                            │
│  ┌──────────────────┐                        │
│  │   GameClient      │                        │
│  │  ├─ login()       │  WebSocket connection  │
│  │  ├─ daily_tasks() │  Daily tasks (marked)  │
│  │  ├─ periodic()    │  Periodic (hangup/jar) │
│  │  └─ disconnect()  │                        │
│  └──────────────────┘                        │
│           │                                  │
│     ┌─────┼─────┐                            │
│     ▼     ▼     ▼                            │
│   BON   Crypto  WebSocket                    │
│  Codec  Encrypt  Client                      │
└──────────────────────────────────────────────┘
           │
           ▼
    Game server (WSS)
```

## Dependencies

| Crate | Purpose |
|-------|------|
| `tokio` | Async runtime (WebSocket, HTTP, timers) |
| `tokio-tungstenite` | WebSocket client (rustls TLS) |
| `reqwest` | HTTP client |
| `serde` / `serde_json` | JSON serialization |
| `serde_yaml` | YAML configuration parsing |
| `clap` | CLI argument parsing |
| `chrono` | Date and time handling |
| `lz4_flex` | LZ4 compression/decompression |
| `md-5` | MD5 hash (token ID) |
| `rand` | Random numbers (encryption) |
| `base64` | Base64 encoding/decoding |
| `qrcode` | Terminal QR-code generation |
| `rqrr` | QR-code image recognition |
| `image` | Image decoding |
