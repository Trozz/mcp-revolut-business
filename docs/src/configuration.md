# Configuration reference

The server is configured by environment variables. Any variable can be given as a file instead,
by adding `_FILE`: `REVOLUT_PRIVATE_KEY_FILE=/keys/revolut-private.pem`.

## Revolut

| Variable | Default | |
|---|---|---|
| `REVOLUT_API_KEY` | required | The API key, `r_live_...` or `r_test_...`. |
| `REVOLUT_PRIVATE_KEY` | required | The Ed25519 private key registered with the key, PEM (`BEGIN PRIVATE KEY`). A key pasted on one line with literal `\n` is read too. |
| `REVOLUT_ENVIRONMENT` | from the key | `production` or `sandbox`. An `r_test_` key means sandbox, an `r_live_` key production; a mismatch stops the server. |
| `REVOLUT_API_URL` | Revolut's | The API base, ending `/api/1.0`. Only for testing. |
| `REVOLUT_ALLOW_PAYMENTS` | `false` | Offer `create_payment`, `transfer_between_accounts` and `exchange_currency`, which move money at once. |

## The server

| Variable | Default | |
|---|---|---|
| `PUBLIC_URL` | required | The external URL clients use, without a path. |
| `OAUTH_SIGNING_KEY` | required | At least 32 bytes (`openssl rand -hex 32`). Changing it signs you out. |
| `OAUTH_ADMIN_PASSWORD` | required | The password you sign in with, at least 12 characters. |
| `MCP_AUTH_TOKEN` | none | A fixed bearer token for clients that cannot do OAuth. Use a long random value. |
| `REVOLUT_READ_ONLY` | none | `revolut` or `all`: hide and refuse every tool that changes anything. |
| `REVOLUT_COMBINED` | `true` | Also serve the tools at `/mcp`, prefixed `revolut_`. |
| `AUDIT_LOG_PATH` | none | Append a JSON line for every write tool call. |
| `BIND_ADDR` | `0.0.0.0:8443` | Listen address. |
| `TLS_CERT_PATH`, `TLS_KEY_PATH` | none | PEM certificate chain and key. Without them, a self-signed certificate. |
| `INSECURE_HTTP` | `false` | Plain HTTP, behind a proxy that terminates TLS. |
| `LOG_LEVEL`, `LOG_FORMAT` | `info`, `json` | Logging. |
| `METRICS_ADDR` | none | Serve Prometheus metrics, e.g. `0.0.0.0:9090`. |

The rest of the server's settings (proxies, origins, tracing) are as in
[mcp-server-core](https://github.com/Trozz/mcp-server-core-rs#configuration).
