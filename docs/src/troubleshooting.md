# Troubleshooting

Tool errors start with a kind, which says what to do.

| Kind | Meaning |
|---|---|
| `auth` | Revolut refused the API key or its signature. Check `REVOLUT_API_KEY`, that `REVOLUT_PRIVATE_KEY` is the key whose public half was uploaded, that the key is still active, and that the server's clock is right (signatures carry the time). |
| `permission` | The key's access does not cover this, e.g. `The following scopes are required: [counterparties:read]`, or the IP allowlist does not include the server. Change the key in Revolut Business, Settings, APIs. |
| `disabled` | A direct payment tool was called without `REVOLUT_ALLOW_PAYMENTS`. Use a payment draft. |
| `rate_limited` | Revolut asked to slow down. Wait and try again. |
| `not_found` | No such id. In the sandbox, a payment draft may not be readable straight after it is created. |
| `validation` | The arguments were wrong, or Revolut rejected them; the message says why. |
| `upstream` | Revolut could not be reached or failed. For a payment, check whether it went through before retrying. |

The server checks its configuration at start and reports every problem at once, for example an
`r_test_` key with `REVOLUT_ENVIRONMENT=production`, or a private key that is not Ed25519.
