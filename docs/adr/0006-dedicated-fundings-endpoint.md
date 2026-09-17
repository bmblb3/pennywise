# Add a dedicated `POST /fundings` + `GET /fundings` pair

[ADR 0004](0004-envelope-funding-via-sentinel-account.md) deliberately rejected a dedicated funding endpoint: "this API's whole surface is already 'everything is a Transaction' ... a parallel non-Transaction concept would be the only exception to that uniformity." That reasoning still holds for the *model* — funding an Envelope is still, underneath, an ordinary Transaction against the sentinel Account (id `0`). What changed is the *write ergonomics*: calling `POST /transactions` to fund an Envelope requires the caller to know the sentinel account id, supply their own dedup id, and pass a signed outflow amount (`-20.00` to reserve `20.00`) — three details specific to this one Transaction shape that a caller has to get right every time.

## Decision

`POST /fundings` is a thin wrapper, not a new concept: it takes `{account_id, envelope_id, amount, date, description}` — `envelope_id` required (unlike `TransactionCreate`, where it's optional) and `amount` an unsigned magnitude ("reserve this much") — generates an id server-side, sets `opposing_account_id: 0`, negates `amount`, and calls the same `insert_transaction` path `POST /transactions` uses. The `201` response is a plain `Transaction`, identical in shape to `POST /transactions`'s.

`GET /fundings` is `GET /ledger`'s mirror image: same denormalized-names idea (`account`, `envelope`, `currency`), but selects `is_funding` rows instead of excluding them, and reports `amount` as the positive magnitude reserved (negating the stored outflow) to match what `POST /fundings` accepts.

`POST /transactions` with `opposing_account_id: 0` keeps working exactly as before — this doesn't replace or deprecate it, it adds a purpose-built path alongside the general one, same relationship `POST /fundings` has to `POST /transactions/batch`.

## Consequences

- **No dedup on `POST /fundings`.** `TransactionCreate.id` is "caller-supplied id (e.g. hash of bank id); sole dedup mechanism" — that guarantee is specific to bank-imported Transactions and doesn't apply here, since fundings are direct user actions with no external id to hash. `POST /fundings` generates its own id, so calling it twice with the same body creates two separate reservations, not a `409`. A client that needs idempotent funding should use `POST /transactions` directly with its own id.
- **The uniformity ADR 0004 argued for is narrowed, not abandoned.** There is now one special-cased write path (`POST /fundings`), but it doesn't introduce a second data model — a funding row created via `POST /fundings` is indistinguishable from one created via `POST /transactions` once it's in the table. `GET /transactions`, `GET /ledger`'s exclusion, `envelope_balances`, and `account_headroom` are all unchanged.
- **`GET /fundings`'s sign convention is the mirror of `GET /ledger`'s**, not an extension of it: `GET /ledger` reports amounts signed from each account's perspective (matching `TransactionCreate.amount`); `GET /fundings` always reports a positive "amount reserved" (matching `CreateFundingRequest.amount`), because every row it returns is, by construction, an outflow into the sentinel account — there's no sign ambiguity to preserve.
