# Getting started

## 1. Create an API key

In Revolut Business, open **Settings, APIs** and create an API key. Revolut asks for the public
half of an Ed25519 key pair:

```sh
openssl genpkey -algorithm ed25519 -out revolut-private.pem
openssl pkey -in revolut-private.pem -pubout -out revolut-public.pem
```

Upload `revolut-public.pem`, and give the key only the access you need: a key that can only read
accounts and transactions is enough for reporting. Keep `revolut-private.pem` secret; the server
needs it.

Try everything in the sandbox first: sign in at `sandbox-business.revolut.com` and create a
sandbox key, which starts `r_test_`. The server then talks to the sandbox on its own.

## 2. Run the server

```sh
docker run -d --name mcp-revolut-business -p 8443:8443 \
  -e PUBLIC_URL=https://bank-mcp.example.com \
  -e OAUTH_SIGNING_KEY="$(openssl rand -hex 32)" \
  -e OAUTH_ADMIN_PASSWORD='a long password' \
  -e REVOLUT_API_KEY=r_test_... \
  -e REVOLUT_PRIVATE_KEY_FILE=/keys/revolut-private.pem \
  -v "$PWD/keys:/keys:ro" \
  ghcr.io/trozz/mcp-revolut-business:latest
```

For a local try without a certificate, set `PUBLIC_URL=http://localhost:8443` and
`INSECURE_HTTP=true`. claude.ai needs a certificate it trusts: give `TLS_CERT_PATH` and
`TLS_KEY_PATH`, or put the server behind a proxy that terminates TLS.

## 3. Connect a client

- **claude.ai**: Settings, Connectors, Add custom connector, with
  `https://bank-mcp.example.com/revolut`. Sign in with the password.
- **Claude Code**:
  `claude mcp add --transport http revolut https://bank-mcp.example.com/revolut`, then `/mcp` to
  sign in.

Ask "how do our Revolut accounts stand?" to check it works.
