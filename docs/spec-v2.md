# Pennywise v2 spec

v2 adds exactly four things on top of [v1](spec-v1.md): `GET /balances`, `DELETE /batches/{id}`, piggy banks, and tags. v1 is frozen — it stays as the historical record of what shipped first, and everything here is additive. See `CONTEXT.md` for the vocabulary this spec uses.

> **Draft.** Sections land one at a time and get reconciled into a single document (naming, ordering, updated non-goals) once all four are written.

`docs/openapi.yaml` documents the API as it is actually served, so it is **not** updated ahead of implementation — each v2 endpoint lands there in the commit that builds it. Until then the request/response shapes live here, which is why this spec states them inline rather than deferring to the OpenAPI document the way v1 does.

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

### `DELETE /batches/{id}`

Removes every Transaction carrying `batch_id = {id}`. The symmetric undo of `POST /transactions/batch`: an import went in as a unit, it comes out as a unit.

- **`204` on success**, no body — matching `DELETE /transactions/{id}` and `DELETE /accounts/{id}`.
- **`404` when no Transaction carries that `batch_id`.** This also covers the second call, so the status code is not idempotent and "already deleted" is indistinguishable from "never existed". Accepted: there is no batch registry to consult, only the rows themselves.
- **No `400` case.** Nothing in the schema references a Transaction, so no foreign key can block the delete.
- **Hard delete.** No soft-delete flag, no tombstone, and no "has this been edited since import?" check — v1 has no field that could answer that reliably (`updated_at` moves for reasons unrelated to a correction), and v2 isn't adding one.
- **`{id}` is caller-supplied text**, like a Transaction id, and the router does not percent-decode path segments: a batch id containing `/` or `%` will not round-trip. This is an existing constraint inherited from `DELETE /transactions/{id}`; the fix is to choose URL-safe batch ids, not to add decoding.

#### What a batch actually is

`batch_id` is a plain caller-supplied `TEXT` column with no uniqueness constraint and no `batches` table behind it — a Batch exists only as "the set of rows carrying this string". Two consequences worth stating rather than leaving to be discovered:

- `POST /transactions` accepts a `batch_id` as well, so a Batch is not necessarily one `POST /transactions/batch` call. Whatever carries the string is deleted, regardless of how it was inserted.
- There is no `GET /batches` to discover ids. The caller minted the `batch_id` on the way in, so remembering it is the caller's job. A listing endpoint is not part of v2.

#### Implementation

A single `DELETE FROM transactions WHERE batch_id = ?1`, with the affected-row count choosing `204` over `404` — the same shape as `delete_transaction`. A lone `DELETE` is already its own implicit SQLite transaction, so it is atomic without an explicit `BEGIN`; don't wrap it in one, and don't `SELECT` first to check existence.

No schema change and no migration: `batch_id` has been populated on every import since v1, with no endpoint acting on it. **No index on `batch_id` either** — a full scan across one person's ledger, on an operation run by hand a few times a year, is not worth an index that every insert then has to maintain.

Deleting a Batch frees every Transaction `id` in it, so re-importing the same source file re-creates those Transactions (`CONTEXT.md`: a Transaction's `id` is the sole dedup mechanism, and deletion is not durable). Here that is the point rather than a caveat — the expected repair loop is delete the bad import, fix the importer, import again.
