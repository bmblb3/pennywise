# Currency stays on the Account, not the Transaction

We considered moving Currency from Own Account to Transaction (see the reverted commit ff2f0cc), to let a single Own Account hold balances in more than one Currency. The motivating case was real: cash in a drawer, held in more than one currency. But cash doesn't pool — a SEK note and a USD note never merge into one fungible balance the way a real multi-currency account (Wise, Revolut) does; they just sit in the same drawer without mixing. That case is already representable under the original schema by giving each currency its own Own Account (`Cash (SEK)`, `Cash (USD)`), which is also how multi-currency holdings are conventionally modeled in double-entry ledgers (Beancount, ledger-cli, GnuCash): one account per currency.

Moving Currency onto the Transaction would have bought representability for an account type we don't hold (a real pooled multi-currency account) at the cost of a real invariant: today, an Own Account's Currency is enforced once at the account level, so every Transaction against it is correct by construction. Per-transaction Currency drops that to "whatever the caller happened to send," with nothing in the schema to catch a mistyped code. That's a worse trade for a problem we don't have yet.

## Consequences

If a genuine pooled multi-currency account (e.g. an actual Wise account) shows up later, this decision gets revisited then, with a real account to design against instead of a hypothetical one. Until then, `accounts.currency_id` stays required for `own` accounts, and multiple currencies held under one conceptual name (e.g. cash) are modeled as multiple Own Accounts.
