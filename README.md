# mcp-revolut-business

A self-hosted [Model Context Protocol](https://modelcontextprotocol.io) server for
[Revolut Business](https://www.revolut.com/business/). It lets Claude and other MCP clients read
your accounts, transactions, counterparties, cards and team, and prepare payments for you to
approve in the Revolut Business app.

It is built on [mcp-server-core](https://github.com/Trozz/mcp-server-core-rs), so it serves
Streamable HTTP with OAuth 2.1 sign-in, TLS, read-only mode, an audit log and metrics.

**Documentation: <https://trozz.github.io/mcp-revolut-business/>**

## How it works

Revolut Business API keys (Settings, APIs, in Revolut Business) belong to one business. The
server holds the key and the Ed25519 private key registered with it, and signs every request:

- `Authorization: Bearer <api key>`;
- `X-Revx-Timestamp`: the time in Unix milliseconds;
- `X-Revx-Signature`: the base64 Ed25519 signature of timestamp + method + path + query + body.

You sign in to the server with its owner password, so only you can use the key.

**Money does not move without you.** By default the server cannot pay anyone directly: it
prepares *payment drafts*, which someone with approval rights approves in the Revolut Business
app. The tools that pay, transfer and exchange at once exist only when
`REVOLUT_ALLOW_PAYMENTS=true`, and every payment carries a `request_id` so a retry cannot pay
twice.

## Quick start

1. In Revolut Business, open **Settings, APIs** and create an API key. Generate its key pair:

   ```sh
   openssl genpkey -algorithm ed25519 -out revolut-private.pem
   openssl pkey -in revolut-private.pem -pubout -out revolut-public.pem
   ```

   Upload `revolut-public.pem` with the key and give it only the access you need. Try it in the
   sandbox first (sandbox-business.revolut.com, keys starting `r_test_`).

2. Run the server:

   ```sh
   docker run -d --name mcp-revolut-business -p 8443:8443 \
     -e PUBLIC_URL=https://bank-mcp.example.com \
     -e OAUTH_SIGNING_KEY="$(openssl rand -hex 32)" \
     -e OAUTH_ADMIN_PASSWORD='a long password' \
     -e REVOLUT_API_KEY=r_live_... \
     -e REVOLUT_PRIVATE_KEY_FILE=/keys/revolut-private.pem \
     -v "$PWD/keys:/keys:ro" \
     ghcr.io/trozz/mcp-revolut-business:latest
   ```

   Or from source: `cargo run --release -p mcp-revolut-business` with the same variables
   (`INSECURE_HTTP=true` and `PUBLIC_URL=http://localhost:8443` for a local try).

3. Connect a client:
   - **claude.ai**: Settings, Connectors, Add custom connector:
     `https://bank-mcp.example.com/revolut`, then sign in with the password.
   - **Claude Code**: `claude mcp add --transport http revolut https://bank-mcp.example.com/revolut`,
     then `/mcp` to sign in.

## Configuration

Every variable can also be given as a file with `_FILE`, e.g. `REVOLUT_PRIVATE_KEY_FILE`.

| Variable | Default | |
|---|---|---|
| `REVOLUT_API_KEY` | required | The API key, `r_live_...` or `r_test_...`. |
| `REVOLUT_PRIVATE_KEY` | required | The Ed25519 private key registered with it, PEM (`BEGIN PRIVATE KEY`). |
| `REVOLUT_ENVIRONMENT` | from the key | `production` or `sandbox`. An `r_test_` key means sandbox; a mismatch is an error. |
| `REVOLUT_API_URL` | Revolut's | The API base, ending `/api/1.0`. |
| `REVOLUT_ALLOW_PAYMENTS` | `false` | Offer `create_payment`, `transfer_between_accounts` and `exchange_currency`, which move money at once. |
| `REVOLUT_READ_ONLY` | none | `revolut` or `all`: hide and refuse every tool that changes anything. |
| `REVOLUT_COMBINED` | `true` | Also serve the tools at `/mcp`, prefixed `revolut_`. |
| `PUBLIC_URL` | required | The external URL, without a path. |
| `OAUTH_SIGNING_KEY` | required | At least 32 bytes (`openssl rand -hex 32`). |
| `OAUTH_ADMIN_PASSWORD` | required | The password you sign in with, at least 12 characters. |
| `MCP_AUTH_TOKEN` | none | A fixed bearer token for clients that cannot do OAuth. |
| `AUDIT_LOG_PATH` | none | Append a JSON line for every write tool call. |

TLS, proxies, logging, metrics and tracing are configured as in
[mcp-server-core](https://github.com/Trozz/mcp-server-core-rs#configuration).

## Tools

| Area | Tools |
|---|---|
| Overview | `business_snapshot` |
| Accounts | `list_accounts`, `get_account`, `get_account_bank_details`, `get_exchange_rate` |
| Transactions | `list_transactions`, `get_transaction` |
| Statements | `summarise_transactions` (totals, fees, top merchants, worked out on the server), `export_statement` (CSV, including FreeAgent's format) |
| Counterparties | `list_counterparties`, `get_counterparty`, `create_counterparty`, `delete_counterparty`, `validate_account_name` (UK and EUR), `get_counterparty_requirements` |
| Before paying | `get_payment_quote` (fee, rate and arrival), `list_transfer_reasons` |
| Payment drafts | `list_payment_drafts`, `get_payment_draft`, `create_payment_draft`, `delete_payment_draft` |
| Payments (opt-in) | `create_payment`, `transfer_between_accounts`, `exchange_currency` |
| Payout links | `list_payout_links`, `get_payout_link`, `create_payout_link`, `cancel_payout_link` |
| Cards | `list_cards`, `get_card`, `freeze_card`, `unfreeze_card` |
| Card admin | `create_virtual_card`, `update_card`, `lock_card`, `unlock_card`, `terminate_card` |
| Team | `list_team_members`, `get_team_member`, `list_roles`, `list_expenses`, `get_expense` |
| Team admin | `invite_team_member`, `change_team_member_role`, `suspend_team_member`, `unsuspend_team_member`, `remove_team_member` |
| Anything else | `api_get` (read only) |

Prompts: `account_overview`, `monthly_review`, `pay_supplier`, `unusual_spending` and
`statement_for_freeagent`.

All 48 tools are offered unless hidden: the three that move money at once need
`REVOLUT_ALLOW_PAYMENTS=true`, and `REVOLUT_READ_ONLY` hides everything that changes anything.

Card numbers and CVVs are never returned. Text in the account (references, merchant and payee
names) is treated as data, not instructions.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

`tests/live_sandbox.rs` runs the tools against Revolut's real sandbox when given a sandbox key:

```sh
REVOLUT_SANDBOX_API_KEY=r_test_... REVOLUT_SANDBOX_PRIVATE_KEY_FILE=key.pem \
  cargo test -p mcp-revolut-business-banking --test live_sandbox -- --test-threads 1
```

## Licence

MIT
