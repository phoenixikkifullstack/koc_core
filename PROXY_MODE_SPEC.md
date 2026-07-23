# Realtime Proxy Mode Specification

Status: Implemented MVP

## Purpose

Proxy mode captures complete WebSocket messages, decodes the game's XOR/LZ4/BON protocol in realtime, correlates requests with responses, and builds command-shape catalogs that can be used to add new `GameClient` automation flows.

The forwarding path is independent of the decoding path. A malformed or unsupported game message must never modify, delay, or terminate otherwise valid relay traffic.

## Architecture

```text
WebSocket client
      |
      v
koc_proxy relay ---------> upstream game WSS
      |
      | bounded copy, never awaited by forwarding
      v
CaptureRecord
      |
      +--> realtime Inspector output
      +--> optional raw JSONL recorder
      +--> request/response correlator
      +--> optional command catalog
```

`proxy.rs` owns only WebSocket transport. `proxy_capture.rs` owns capture records, protocol inspection, redaction, correlation, and catalog generation. Protocol parsing remains in `protocol.rs`, `crypto.rs`, and `bon.rs`.

## Modes

### Realtime Relay

`koc_proxy relay` listens on `127.0.0.1:8787` by default. It forwards the inbound WebSocket path and query to the configured upstream origin, but it never logs or stores the handshake query.

Decoded output is enabled by default. `--no-decode` disables console output without disabling optional recording or catalog collection.

Raw `--record` output intentionally contains transport metadata and base64 payloads rather than decoded `cmd` or body values. Realtime decoded events are console output; saved raw sessions are decoded with the `decode` subcommand. Catalog output is a single JSON snapshot keyed by command name, even if the operator chooses a `.jsonl` filename.

### External Realtime Stream

`koc_proxy inspect --stream` consumes newline-delimited `CaptureRecord` objects from stdin. This is the integration point for an external TLS MITM. Input payloads must already be complete WebSocket messages with TLS, RFC 6455 framing, fragmentation, and client masking removed.

### Offline Decode

`koc_proxy decode` runs the same Inspector over a saved capture. It exists so parser fixes can be applied to historical unknown messages without repeating live game actions.

### Command Catalog

`koc_proxy catalog` groups observed commands and reports request, response, and push counts; response codes; and distinct JSON field/type shapes. Catalog output contains no raw handshake data.

## Capture Schema

Each JSONL record has schema version 1:

```json
{
  "version": 1,
  "connection_id": "conn-1",
  "frame_index": 42,
  "timestamp_ms": 1784710800000,
  "direction": "client_to_server",
  "opcode": "binary",
  "payload_base64": "cHg..."
}
```

`connection_id` scopes sequence numbers because the game client resets its sequence after reconnecting. `frame_index` preserves observed ordering inside a connection.

## Correlation

- Client requests are indexed by `(connection_id, seq)`.
- Server responses are matched using `(connection_id, resp)`.
- Server messages without `resp` are recorded as pushes.
- Heartbeat messages remain visible but are not treated as normal requests when `seq == 0`.
- Correlation state is bounded and stale pending requests are discarded.

## Protocol Safety

- Wire messages larger than 8 MiB are rejected by the decoder.
- Decompressed messages larger than 32 MiB are rejected.
- Relay WebSocket messages and frames are independently bounded, and client/upstream handshakes have timeouts.
- Capture JSONL lines, connection identifiers, command count, shape count, shape depth, and shape width are bounded.
- BON values enforce limits for recursion depth, collection entries, strings, and binary values.
- Invalid LZ4 data returns a decode error instead of an empty successful payload.
- Missing protocol `code` is represented as absent in inspection output rather than implicit success.
- Decode errors are events; they do not propagate into WebSocket forwarding.

## Traffic Safety

- Relay listeners must use a loopback address.
- Plain `ws://` upstream origins are accepted only on loopback; remote upstreams must use `wss://`.
- Simultaneous relay connections are bounded by `--max-connections`.
- All WebSocket message variants are forwarded without modifying their message payloads; RFC 6455 frame encoding is owned by tungstenite on each side.
- Capture analysis uses a bounded channel and `try_send`; a slow decoder may drop inspection records but cannot backpressure game traffic.
- Dropped-record counts are reported on stderr and in the relay shutdown summary.
- Active injection and live replay are outside this mode. New automation must construct commands through `GameClient`, which owns current sequence, acknowledgement, timeout, and battle-version state.

## Data Safety

- The `p` authentication query is used only to construct the upstream connection and is never emitted as a capture record.
- Capture files are mode `0600` on Unix.
- Existing regular capture and catalog files are truncated when a relay session starts. Symlinks and non-file paths are rejected.
- `captures/` is ignored by Git.
- Common token, session, connection, and identity fields are redacted from decoded output by default.
- `--show-sensitive` affects decoded output only. Raw captures remain sensitive regardless of this flag.

## Automation Feedback Workflow

1. Start `koc_proxy relay` with realtime decoding and optional recording.
2. Point the controlled client at it through `KOC_WS_BASE_URL`.
3. Perform one target action manually.
4. Use the correlated request/response timeline and catalog shapes to identify commands and dynamic fields.
5. Add a typed wrapper to `GameClient` rather than replaying captured bytes.
6. Convert a manually sanitized sample into a protocol fixture.
7. Wire manual CLI verification before adding the command to scheduled automation.

## Acceptance Criteria

- Client and server Binary payloads are forwarded without game protocol decryption or re-encoding.
- `px`, `pl`, and plain BON payloads use the existing protocol implementation.
- Realtime and offline modes produce equivalent decoded events for the same records.
- Responses report their matched request command and observed latency.
- Filtering does not affect correlation or catalog collection.
- Handshake tokens do not appear in capture output or lifecycle logs.
- Parser failures and a full analysis channel do not terminate the relay.
- Unit tests and `cargo check --all-targets` pass without live-account traffic.
