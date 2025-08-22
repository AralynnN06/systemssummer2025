# sitecheck — Concurrent Website Status Checker (Rust)

A concurrent website monitoring tool using **Rust threads and channels**.  
It checks multiple URLs simultaneously, with **timeouts**, **retries**, and optional **periodic monitoring**.  
It reports **HTTP status code, response time, timestamp**, and supports extra validations.

## Features

- Accepts URLs via CLI or file (`-f urls.txt`)
- Thread pool using `std::thread` + `std::sync::mpsc` channels
- Configurable timeout (`--timeout`), worker threads (`--threads`), and retries (`--retries`)
- Collects and reports:
  - HTTP status code (or error)
  - Response time
  - Timestamp (UTC)
- Graceful shutdown (Ctrl+C) — completes current round and exits cleanly
- **Bonus**:
  - Periodic monitoring (`--period SECS`)
  - HTTP header validation (`-H 'Name: Value'`)
  - Basic SSL verification (via TLS defaults in `ureq`)
  - Response body validation (`--contains TEXT`)
  - Statistics (uptime %, average response time)

## Install & Run

```bash
# Build & run with URLs directly
cargo run --release -- https://example.com https://www.rust-lang.org

# Or from file (one URL per line)
cargo run --release -- -f urls.txt -n 80 -t 3 -r 2

# Periodic monitoring every 60s, requiring a header and body content
cargo run --release -- -p 60 -H 'Server: nginx' --contains 'Welcome' https://example.com
```

### Output

Each result is printed as a JSON line, e.g.
```json
{"url":"https://example.com","status":{"Ok":200},"response_time":123,"timestamp":"2025-08-21T23:00:00Z"}
```

A short stats summary follows each round:
```
--- stats summary ---
https://example.com -> checks: 3, uptime: 100.0%, avg_rt_ms: 120.7
---------------------
```

## Testing

This project includes unit/integration tests using `httpmock`.

```bash
cargo test
```

Highlighted tests:
- `test_success_ok` — healthy response with header/body validation
- `test_header_mismatch` — header validation error
- `test_body_contains_validation` — body substring checks
- `test_timeout_error` — request times out
- `test_concurrency_50` — simulates 50 concurrent checks

## Notes

- SSL certificate validation is handled by `ureq` + TLS backend by default. If the handshake or certificate is invalid, the request will fail and be reported as an error.
- For large bodies, consider adding size limits or a streaming check if `--contains` is used in production.
