# Security

- **The keys stay on the server.** The API key and private key are read from the environment or
  files, never logged and never returned by a tool. Give them as files (`_FILE`) readable only by
  the server.
- **Only the owner signs in.** Sign-in needs `OAUTH_ADMIN_PASSWORD`, with the limits of
  mcp-server-core (attempts per address and network, grants that end when the password changes
  and after 90 days).
- **Least access.** Give the API key only the access it needs in Revolut Business, and an IP
  allowlist if the server's address is fixed. Run with `REVOLUT_READ_ONLY=all` for reporting
  only.
- **Payments need a person.** Without `REVOLUT_ALLOW_PAYMENTS`, the server can prepare payment
  drafts but not send them: they are approved in the Revolut Business app, under its approval
  rules. Payout links do move money once claimed, and need the key's pay access.
- **Admin tools are writes.** Issuing, locking or terminating cards and inviting, suspending or
  removing team members all count as changes: `REVOLUT_READ_ONLY` hides them, and the model is
  told to confirm each with you. They also need the API key's access to cards and team members.
- **No double payments.** Writes are never retried by the server, and each payment carries an
  idempotency key.
- **Account text is data.** References, merchant and payee names are written by other people;
  the tools and prompts tell the model to treat them as data, not instructions.
- **Card details.** Card numbers and CVVs are dropped from every result.
- **An audit log.** Set `AUDIT_LOG_PATH` to record every write tool call.
