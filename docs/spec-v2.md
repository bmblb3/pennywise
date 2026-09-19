# Pennywise v2 spec

v2 makes three additions on top of [v1](spec-v1.md) — `GET /balances`, envelopes, and tags — and one removal: `batch_id`. v1 is frozen — it stays as the historical record of what shipped first; v2 is no longer purely additive. See `CONTEXT.md` for the vocabulary this spec uses.

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

This is the only endpoint aggregating real money movement in v2. v1 ruled out "reports or aggregation of any kind"; v2 narrows that to permit own-account balances and nothing else — net worth, category summaries, and per-period totals stay out. (`GET /envelope_balances`/`GET /account_headroom`, added later in this document, are a separate, narrowly-scoped pair over the Envelope mechanism specifically — see ADR 0007 — and don't reopen this.)

## Envelopes

An **Envelope** is a named, virtual reservation of money already sitting in one or more Own Accounts — never a sub-account, never a place money physically moves to. Funding one is an ordinary Transaction against a reserved sentinel Account (id `0`). Full mechanism and alternatives considered: [ADR 0004](adr/0004-envelope-funding-via-sentinel-account.md); delete-behavior rationale: [ADR 0005](adr/0005-restrict-not-set-null-for-envelope-and-category.md). Both are pinned as runnable assertions in [`0004-envelope-funding-via-sentinel-account.verify.sh`](adr/0004-envelope-funding-via-sentinel-account.verify.sh).

### Schema

```sql
CREATE TABLE envelopes (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);   -- mirrors categories exactly

ALTER TABLE transactions ADD COLUMN envelope_id INTEGER REFERENCES envelopes(id);          -- see Cross-table validation below for ON DELETE
ALTER TABLE transactions ADD COLUMN is_funding GENERATED ALWAYS AS (opposing_account_id = 0) VIRTUAL;

CREATE VIEW ledger AS SELECT * FROM postings WHERE NOT is_funding;   -- replaces GET /postings

CREATE VIEW envelope_balances AS
SELECT account_id, envelope_id,
       SUM(CASE WHEN is_funding THEN -amount ELSE amount END) AS balance
FROM postings
WHERE envelope_id IS NOT NULL AND account_id IN (SELECT id FROM accounts WHERE type = 'own')
GROUP BY account_id, envelope_id;

CREATE VIEW account_headroom AS
SELECT account_id, bal, still_reserved, bal - still_reserved AS headroom FROM (
  SELECT a.id AS account_id,
    COALESCE((SELECT SUM(amount) FROM ledger l WHERE l.account_id = a.id), 0) AS bal,
    COALESCE((SELECT SUM(MAX(e.balance, 0)) FROM envelope_balances e WHERE e.account_id = a.id), 0) AS still_reserved
  FROM accounts a WHERE a.type = 'own');
```

- **The sentinel Account (id `0`, name "Envelope") is seeded once at migration time**, before any user-created Account can claim that id. It is an ordinary External Account, visible via `GET /accounts` like any other — no special-casing beyond the reserved id.
- **`is_funding` is a generated column** (`opposing_account_id = 0`) — never a value the API sets or accepts, always derived.
- **`envelope_id` is legal on any Transaction, not just funding rows** — including a Transfer between two Own Accounts, which lets a reservation follow real money from the Account it was funded in to the Account it's actually spent from (verified end-to-end with a credit-card scenario in the harness).
- **An Envelope's balance is per `(account_id, envelope_id)`, never summed across Accounts or Currencies** — `envelope_balances` groups by both, so funding the same Envelope from a EUR, USD, and THB Account is legal; a client wanting a combined total sums the per-Account rows itself, consistent with this API's no-server-side-aggregation stance.
- **Overspending an Envelope is legal** — nothing enforces non-negative balances. An overspent Envelope reports a negative `balance` in `envelope_balances`, but contributes `0` (never negative) to `account_headroom.still_reserved`: an Envelope can't reserve money it doesn't have, and the overspend already shows up as reduced real balance via `GET /ledger`.
- **`account_headroom.headroom` (`bal - still_reserved`) is a signal, not an enforced limit** — funding an Envelope from an Account that doesn't have the money to spare is legal; headroom just goes negative.
- **`envelope_balances` and `account_headroom` are exposed as `GET /envelope_balances` and `GET /account_headroom`** (ADR 0007, reversing this section's original "not exposed via any endpoint" call). `GET /balances` remains the only endpoint aggregating real money movement — these two are a separate, narrowly-scoped pair over the Envelope mechanism specifically; net worth, category summaries, and per-period totals are still out of scope. See "API" below for both shapes.

### Cross-table validation

Added to the shared validation function (`docs/spec-v1.md`'s "Cross-table validation" — same function, no new one):

3. `envelope_id`, if present, must reference an existing Envelope — already enforced natively by the `REFERENCES` clause plus `PRAGMA foreign_keys = ON`, so no additional application-level check is needed beyond what v1's rule 1 already establishes for the pattern.

**Breaking change carried over from v1: `category_id`'s `ON DELETE SET NULL` is reversed to `RESTRICT`.** Neither `category_id` nor `envelope_id` carries an explicit `ON DELETE` clause, so both fall back to SQLite's default (`NO ACTION`, enforced immediately) — the same behavior `accounts` already relies on. Reasoning: an Envelope's balance is a real computed financial signal (`account_headroom` depends on it); silently detaching a Transaction from a deleted Envelope would drift a headroom number with no Transaction to justify the change, the same integrity problem `category_id`'s original `SET NULL` was safe from only because Category is purely descriptive. Full rationale: ADR 0005. Concretely, this means `DELETE /categories/{id}` — an already-shipped, otherwise-unchanged v1 endpoint — now returns `400` (FK violation, per the existing "Cross-table validation" status-code rule) instead of always succeeding with `204`, whenever the Category is still referenced by any Transaction.

### API

- **`POST /envelopes`, `GET /envelopes`, `DELETE /envelopes/{id}`** — CRUD mirrors `categories` exactly: `POST` takes `{"name": "..."}` and returns `201` with `{"id", "name"}` (`400`/`409` on validation failure/duplicate name, same as `POST /categories`); `GET` lists all; `DELETE` returns `204`, or `400` if any Transaction still references the Envelope (the `RESTRICT` above), or `404` for an unknown id.
- **Funding an Envelope stays legal as a plain `POST /transactions`** (or a row inside `POST /transactions/batch`) with `opposing_account_id: 0` and `envelope_id` set — `TransactionCreate` gains an optional, nullable `envelope_id` field alongside the existing `category_id`. This remains the only way to get dedup (`id` is caller-supplied) on a funding row.
- **`POST /fundings`, `GET /fundings`** — a friendlier pair layered on the same mechanism (ADR 0006, reversing ADR 0004's original "no dedicated endpoint" call). `POST` takes `{account_id, envelope_id, amount, date, description}` (`envelope_id` required, `amount` an unsigned magnitude to reserve — not the signed outflow `POST /transactions` expects) and returns `201` with the underlying `Transaction` (id server-generated, `amount` stored as `-amount`, `opposing_account_id: 0`); `400` on a non-positive `amount` or an unknown `account_id`/`envelope_id`. `GET /fundings` lists `is_funding` rows only, denormalized like `GET /ledger` (`account`, `envelope`, `currency`), with `amount` reported positive to match what `POST /fundings` accepts.
- Like `amount`, `date`, and the account ids, `envelope_id` is not on `PATCH /transactions/{id}`'s allowed-fields list — naming it in a PATCH body is a `400`, the same immutable-field rule already applied to every field not explicitly listed as patchable.
- **`GET /postings` is renamed to `GET /ledger`** — same ordering, excludes funding rows (`is_funding`), and gains one new field: a nullable `envelope` name, denormalized the same way `category` already is. Raw visibility into funding Transactions (e.g. for an audit trail) isn't lost: `GET /transactions` already lists every row verbatim, funding included.
- **`envelope` on a ledger row reflects a spend/transfer leg tagged with `envelope_id`, not an Envelope's balance** — funding rows never reach the ledger at all (excluded as `is_funding`), so this field only ever shows up on the "spent from" side of a reservation (e.g. the credit-card scenario in ADR 0004). An Envelope's actual balance is `GET /envelope_balances` below.
- **`GET /envelope_balances`, `GET /account_headroom`** (ADR 0007, reversing this endpoint's original "not exposed" status). `GET /envelope_balances` returns one row per `(account, envelope)` with any tagged Transaction: `{account, envelope, currency, balance}`, denormalized names like `GET /ledger`; `balance` can be negative (overspent). `GET /account_headroom` returns one row per Own Account, including ones with no Envelope activity: `{account_id, account_name, currency, bal, still_reserved, headroom}` — `bal` is the same number `GET /balances` reports for that account, `still_reserved` sums each Envelope's balance on the account floored at `0`, `headroom` is `bal - still_reserved` and can go negative (a signal, not an enforced limit).

### Implementation notes

- **Migration ordering**: create `envelopes`, add `transactions.envelope_id`/`is_funding`, seed the sentinel Account (id `0`) and only then create the `ledger`/`envelope_balances`/`account_headroom` views (a view referencing a not-yet-existing column fails to create) — all as part of the same schema-version bump the other v2 changes ship under.
- **SQLite version**: generated columns (`GENERATED ALWAYS AS ... VIRTUAL`) need SQLite ≥ 3.31 — comfortably covered by the same bundled `rusqlite` version already relied on for the `batch_id` column drop (≥ 3.35, see "Removed" below).
- **`postings` as a SQL view is a modeling convenience for the schema/harness above**, not a commitment to a literal SQL object: v1 computes postings in application code (`postings_from_row`/`list_postings` in `src/main.rs`), not via a SQL view, and the shipped implementation may keep doing so — applying the `is_funding` exclusion and the `envelope_balances`/`account_headroom` aggregation in Rust instead. This spec fixes the resulting behavior, not the SQL objects that produce it.

## Removed

### `batch_id`

`batch_id` is dropped entirely — a breaking change to the already-shipped v1 API, carried out in v2 rather than left for v1 to decide. See [ADR 0003](adr/0003-drop-batch-id.md).

- **Schema migration.** `ALTER TABLE transactions DROP COLUMN batch_id` — `rusqlite` 0.31 bundles a SQLite new enough to drop a column directly, so this is a plain migration, not the create-copy-drop-rename dance v1's `CHECK`-adding migration needed.
- **`POST /transactions` and `POST /transactions/batch` no longer accept or return `batch_id`.** The field disappears from both request and response shapes on both endpoints.
- **`POST /transactions/batch` otherwise keeps its name, path, and atomic-rollback behavior unchanged.** It's still one atomic SQLite transaction, rolled back together on any invalid or duplicate row — "batch" in the name is now plain English, not a reference to a stored grouping.
- **No export or migration of existing values.** Existing rows' `batch_id` values are dropped with the column, unread — no endpoint ever read that grouping back out.
- **`DELETE /batches/{id}` does not ship.** It was spec'd on the premise that `batch_id` needed a consumer; with the column gone, so is the endpoint and the domain concept of a Batch (`CONTEXT.md`).
