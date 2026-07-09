# Dependencies

## Core Dependencies

| Crate | Version | License | Purpose | Rationale |
|-------|---------|---------|---------|-----------|
| `reqwest` | 0.12 | MIT/Apache-2.0 | HTTP client for Gmail API | Standard Rust HTTP client, well-maintained |
| `async-openai` | 0.41 | MIT | LLM client (OpenAI-compatible) | Best local endpoint support, streaming, configurable base URL |
| `rusqlite` | 0.40 | MIT | SQLite database | Embedded, zero deps with `bundled`, handles millions of rows |
| `tokio` | 1 | MIT | Async runtime | Industry standard, required by reqwest and async-openai |
| `serde` | 1 | MIT/Apache-2.0 | Serialization | De facto standard for JSON/TOML |
| `serde_json` | 1 | MIT/Apache-2.0 | JSON | Used everywhere |
| `keyring` | 3 | MIT | OS keychain storage | Cross-platform credential storage |
| `chrono` | 0.4 | MIT/Apache-2.0 | Date/time | Standard date library |
| `regex` | 1 | MIT/Apache-2.0 | Regular expressions | For email condition matching |
| `thiserror` | 2 | MIT/Apache-2.0 | Error derive | Ergonomic error types |
| `tracing` | 0.1 | MIT | Logging | Structured logging |
| `base64` | 0.22 | MIT/Apache-2.0 | Base64url decoding | For Gmail API body decoding |

## Tauri Dependencies

| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| `tauri` | 2.0 | MIT/Apache-2.0 | Desktop app framework |
| `tauri-plugin-notification` | 2.0 | MIT/Apache-2.0 | Desktop notifications |

## Dev Dependencies

| Crate | Version | License | Purpose |
|-------|---------|---------|---------|
| `cargo-deny` | — | — | License auditing |
| `cargo-audit` | — | — | Security auditing |

## Frontend Dependencies

| Package | Version | License | Purpose |
|---------|---------|---------|---------|
| `react` | 19 | MIT | UI framework |
| `react-dom` | 19 | MIT | React DOM |
| `@tauri-apps/api` | 2.0 | MIT | Tauri IPC |
| `@tauri-apps/plugin-notification` | 2.0 | MIT | Tauri notifications |
| `tailwindcss` | 4 | MIT | CSS framework |
| `vite` | 6 | MIT | Build tool |
| `typescript` | 5.x | Apache-2.0 | Type safety |

## License Compatibility

All dependencies use permissive licenses (MIT, Apache-2.0, BSD). No GPL/AGPL dependencies.

The project itself will be licensed under MIT.

## Auditing

```bash
# Check all licenses
cargo deny check licenses

# Check for security vulnerabilities
cargo audit

# List all dependency licenses
cargo license
```

## Version Pinning Strategy

- **Major versions**: Pin to major version (e.g., `reqwest = "0.12"`)
- **Patch versions**: Let Cargo resolve (lock file handles reproducibility)
- **Lock file**: Commit `Cargo.lock` to git for reproducible builds

## Feature Flags

### rusqlite

```toml
rusqlite = { version = "0.40", features = ["bundled", "serde_json"] }
```

- `bundled`: Compiles SQLite into the binary (zero system deps)
- `serde_json`: Automatic JSON serialization for SQLite values

### async-openai

```toml
async-openai = { version = "0.41", features = ["chat", "middleware"] }
```

- `chat`: Chat completion API
- `middleware`: Tower middleware for custom interceptors (retry, logging)

### reqwest

```toml
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }
```

- `json`: JSON request/response
- `rustls-tls`: TLS via rustls (no OpenSSL dependency)
- `default-features = false`: Avoids pulling in default features we don't need
