# Tools and prompts

Ids are UUIDs that Revolut gives: the tools never guess one, they find it with a `list_` tool
first. Amounts are decimals in the account's currency (`1250.50`), currencies ISO 4217 codes,
dates `YYYY-MM-DD`.

## Reading

| Tool | |
|---|---|
| `business_snapshot` | Balances by currency, each active account, the latest transactions and drafts awaiting approval. |
| `list_accounts`, `get_account` | Accounts and balances. |
| `get_account_bank_details` | Account number and sort code, IBAN and BIC, and the payment schemes each supports. |
| `get_exchange_rate` | Revolut's rate and fee between two currencies. Nothing is exchanged. |
| `list_transactions`, `get_transaction` | Statements across accounts, newest first, filtered by dates, account, type, state or request id. |
| `list_counterparties`, `get_counterparty` | Saved payees and their accounts. |
| `list_payment_drafts`, `get_payment_draft` | Drafts waiting for approval. |
| `list_payout_links`, `get_payout_link` | Payout links and their state. |
| `list_cards`, `get_card` | Cards, limits and controls. Never the card number or CVV. |
| `list_team_members` | Team members and roles. |
| `list_expenses`, `get_expense` | Submitted expenses (live accounts only: the sandbox has none). |
| `api_get` | Any other endpoint, read only. |

## Changing things

Every tool below asks the model to show you what it will do and wait for your agreement.

| Tool | |
|---|---|
| `validate_account_name` | Confirmation of Payee for a UK account: does the name match? Nothing is saved. |
| `create_counterparty`, `delete_counterparty` | Save or remove a payee. |
| `create_payment_draft`, `delete_payment_draft` | Prepare payments that someone approves in the Revolut Business app. |
| `create_payout_link`, `cancel_payout_link` | A link someone uses to collect money. |
| `freeze_card`, `unfreeze_card` | Stop a card being used, and allow it again. |

## Moving money at once (opt-in)

Only with `REVOLUT_ALLOW_PAYMENTS=true`:

| Tool | |
|---|---|
| `create_payment` | Pay a counterparty now. |
| `transfer_between_accounts` | Move money between the business's own accounts in one currency. |
| `exchange_currency` | Exchange between currencies at Revolut's rate. |

Each sends a `request_id` and returns it. Revolut makes a payment once per `request_id`, so if a
call fails without an answer, look it up with `list_transactions` (`request_id` and `from`) or
call again with the same `request_id`: it cannot pay twice.

## Prompts

| Prompt | |
|---|---|
| `account_overview` | A short briefing: balances, the last 30 days, anything pending, drafts waiting. |
| `monthly_review` | One month's money in and out, largest payments, fees, card spend, and failures. |
| `pay_supplier` | Pay someone from a plain description, through a checked payee and a payment draft. |
| `unusual_spending` | New merchants, large or repeated charges, foreign or night-time spend, declines. |
