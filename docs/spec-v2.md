# Pennywise v2 spec

v2 makes three additions on top of [v1](spec-v1.md) — `GET /balances`, piggy banks, and tags — and one removal: `batch_id`. v1 is frozen — it stays as the historical record of what shipped first; v2 is no longer purely additive. See `CONTEXT.md` for the vocabulary this spec uses.

> **Draft.** Sections land one at a time and get reconciled into a single document (naming, ordering, updated non-goals) once all four are written.

`docs/openapi.json` documents the API as it is actually served, so it is **not** updated ahead of implementation — each v2 endpoint lands there in the commit that builds it. Until then the request/response shapes live here, which is why this spec states them inline rather than deferring to the OpenAPI document the way v1 does.

## API

### `GET /balances`

The current balance of every Own Account, one row each:

```json
[
  {"account_id": 1, "account_name": "ABCD", "balance": 100.0, "currency": "SEK"}
]
```

- **Own accounts only.** An External Account carries no Currency and isn't the owner's money to total, so it gets no row — exactly as it gets no posting row in `GET /postings`.
- **Every own account appears**, including one with no Transactions at all, with `balance` of `0` rather than a missing row. A client listing accounts shouldn't have to special-case "never used".
- **`balance` is in major units** (e.g. `12.34`), same as `amount` everywhere else in the API. Minor units are a storage representation, not a wire format.
- **`account_id` is included**, where `GET /postings` is deliberately name-only: postings is built to be self-contained for `jq` pipelines, whereas a balance is something a client joins back to an account it already holds. `account_name` rides along so the response stays readable on its own.
- **Ordered by `account_id`.** Any stable order would do; id is the one that's free.
- **No filtering, pagination, or date-ranging** — v1's "no filters anywhere" stance carries forward. In particular there is no "balance as of date" parameter; a client that wants one computes it from `GET /postings`.

#### What the balance is

`balance` is the sum of that account's rows in `GET /postings`. Postings is the definition here, not an implementation coincidence: it already emits one correctly-signed row per own-account touch, with transfer legs split out, cross-currency opposing amounts applied, and currency resolved. Defining balances on top of it keeps the sign and cross-currency rules in exactly one place, and guarantees `GET /balances` can never disagree with what a client gets by summing `GET /postings` itself.

Concretely, an account's balance takes a contribution from:

- every Transaction where it is the `account_id` — `amount` as stored, already signed from its perspective;
- every Transaction where it is the `opposing_account_id` **and** both accounts are `own` (a Transfer) — `-amount`, or, when the Transfer is cross-currency, `opposing_amount` carrying the opposite sign to `amount`.

An own-to-external Transaction contributes only through the first case: the External Account has no balance to move.

#### Summing

Sum in **minor units** (`INTEGER`) and convert to major units only when serializing the response, reusing the same per-row conversion `GET /postings` applies. Summing the major-unit floats instead would reintroduce exactly the drift the `INTEGER amount` column exists to prevent, and a balance is the number most likely to be trusted down to the last unit.

This is the only aggregation endpoint in v2. v1 ruled out "reports or aggregation of any kind"; v2 narrows that to permit own-account balances and nothing else — net worth, category summaries, and per-period totals stay out.

## Removed

### `batch_id`

`batch_id` is dropped entirely — a breaking change to the already-shipped v1 API, carried out in v2 rather than left for v1 to decide. See [ADR 0003](adr/0003-drop-batch-id.md).

- **Schema migration.** `ALTER TABLE transactions DROP COLUMN batch_id` — `rusqlite` 0.31 bundles a SQLite new enough to drop a column directly, so this is a plain migration, not the create-copy-drop-rename dance v1's `CHECK`-adding migration needed.
- **`POST /transactions` and `POST /transactions/batch` no longer accept or return `batch_id`.** The field disappears from both request and response shapes on both endpoints.
- **`POST /transactions/batch` otherwise keeps its name, path, and atomic-rollback behavior unchanged.** It's still one atomic SQLite transaction, rolled back together on any invalid or duplicate row — "batch" in the name is now plain English, not a reference to a stored grouping.
- **No export or migration of existing values.** Existing rows' `batch_id` values are dropped with the column, unread — no endpoint ever read that grouping back out.
- **`DELETE /batches/{id}` does not ship.** It was spec'd on the premise that `batch_id` needed a consumer; with the column gone, so is the endpoint and the domain concept of a Batch (`CONTEXT.md`).
