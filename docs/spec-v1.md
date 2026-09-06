# Pennywise v1 spec

A minimal, self-hosted, single-user personal finance ledger. Single Rust binary, single SQLite file, no auth, no UI, no aggregation. See `CONTEXT.md` for the vocabulary this spec uses (Account, Transaction, Batch, etc.).

## Non-goals (v1)

Explicitly not built in v1 — no stub, trait, or empty module for any of these:

- Reports or aggregation of any kind: no per-account balance, no net worth, no summaries, no charts
- Query filters or pagination on any list endpoint
- Account merge, transaction split
- `DELETE /batches/{id}` (batch undo) — the `batch_id` column is populated on every import, but no endpoint acts on it yet
- Editing accounts, categories, or currencies after creation (no `PATCH`, beyond what's listed under Transactions below)
- Piggy banks, savings goals, tags, bills, recurring transactions, reminders, budgets, envelopes, rollovers
- Multi-currency net worth or exchange-rate fetching
- Bank sync (Plaid/Nordigen/etc.)
- A UI of any kind — API only
- Mobile app, multi-tenant/multi-user, authentication
- Config file or environment variables — two CLI flags cover v1
- Deployment tooling (cross-compilation, systemd units) — "runs on a Pi" is satisfied by the architecture (sync, minimal deps, one connection), not by tooling in this repo

## Schema

The table shapes below reflect one substantive change from the original design: Currency moved from the Account to the Transaction (see [ADR-0002](adr/0002-currency-on-transaction.md)) — `accounts.currency_id` is gone; `transactions` gained `currency_id` and `opposing_currency_id`. Two smaller additions remain from the original pass: `amount`/`opposing_amount` are `INTEGER` minor units rather than `REAL` (float drift makes a ledger's core numbers untrustworthy), and `currencies` carries `minor_unit` so the file is self-describing to any consumer that has to format an amount without a hardcoded ISO 4217 table. `categories.category_id`'s `ON DELETE SET NULL` is likewise new — it's required to realize "category deletion never breaks a transaction," which has no default in SQLite.

```sql
CREATE TABLE currencies (
    id         INTEGER PRIMARY KEY,
    code       TEXT NOT NULL UNIQUE,                          -- ISO 4217, e.g. 'SEK', 'USD', 'EUR'
    minor_unit INTEGER NOT NULL CHECK (minor_unit BETWEEN 0 AND 4)  -- decimal places: 2 for USD, 0 for JPY
);

CREATE TABLE accounts (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('own','external'))
    -- no currency here: an account (own or external) may see transactions in more than one currency
);

CREATE TABLE categories (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE transactions (
    id                    TEXT PRIMARY KEY,   -- caller-supplied (e.g. hash of bank id); sole dedup mechanism
    batch_id              TEXT,               -- caller-supplied; groups an import for future bulk undo. NULL for standalone entries.
    description           TEXT NOT NULL,
    date                  TEXT NOT NULL,
    amount                INTEGER NOT NULL CHECK (amount != 0),               -- signed minor units; +ve = inflow to account_id
    currency_id           INTEGER NOT NULL REFERENCES currencies(id),        -- currency of `amount`
    opposing_amount       INTEGER CHECK (opposing_amount IS NULL OR opposing_amount > 0),  -- unsigned magnitude only
    opposing_currency_id  INTEGER REFERENCES currencies(id),                  -- NULL means "same as currency_id"
    account_id            INTEGER NOT NULL REFERENCES accounts(id),
    opposing_account_id   INTEGER NOT NULL REFERENCES accounts(id),
    category_id           INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    created_at            TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at            TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK ((opposing_amount IS NULL) = (opposing_currency_id IS NULL)),
    CHECK (opposing_currency_id IS NULL OR opposing_currency_id != currency_id)
);
```

`account_id`/`opposing_account_id` references need no explicit `ON DELETE` clause: SQLite's default (`NO ACTION`, enforced immediately since nothing here defers foreign keys) already rejects deleting an account that transactions still point at, which is the desired behavior.

### Connection setup

Every connection opens with:

```sql
PRAGMA foreign_keys = ON;
```

SQLite does not enforce `REFERENCES` clauses without this — off by default, it would make every FK above decorative.

### Migrations

On startup, read `PRAGMA user_version`. If `0`, run the embedded schema above once and set it to `1`. If `1`, do nothing. No migration framework — the schema is finalized, so v1 has exactly one migration; add a framework at migration three, not migration one.

## Cross-table validation

Only one rule can't be expressed as a plain `CHECK` (it needs a join, which SQLite `CHECK` can't do). Implement it in **one shared function** that every insert/update path calls:

1. `account_id` must reference an account with `type = 'own'`.

The currency-mismatch rule that used to require this join is gone: since `currency_id`/`opposing_currency_id` now live on the transaction itself, the schema's own `CHECK` constraints enforce it — `opposing_amount` and `opposing_currency_id` rise and fall together, and `opposing_currency_id` is never redundantly set equal to `currency_id`. `opposing_amount`, when present, is a magnitude only — its sign is always inferred from `amount`'s sign, never stored separately. Everything else (an account/category id existing at all) is enforced natively by `REFERENCES` plus `PRAGMA foreign_keys = ON`.

## API

HTTP + JSON. No filtering, no pagination, no aggregation on any endpoint — a list endpoint returns everything; a consumer that needs a subset or a sum computes it itself.

Standard envelope: `{"error": "<message>"}` on failure. Status codes: `201` create, `200` read, `204` delete, `400` validation failure (including a `CHECK`/`FK` violation surfaced from SQLite, and an attempt to `PATCH` an immutable field), `404` unknown id, `409` duplicate `id` on insert (primary-key conflict — this **is** the dedup mechanism; don't build a second one).

### Currencies

- `POST /currencies` — `{code, minor_unit}`
- `GET /currencies` — full list

### Accounts

- `POST /accounts` — `{name, type}` — no currency; an account may see transactions in more than one
- `GET /accounts` — full list
- `DELETE /accounts/{id}` — `400` if any transaction still references it (native FK rejection)

### Categories

- `POST /categories` — `{name}`
- `GET /categories` — full list
- `DELETE /categories/{id}` — referencing transactions have `category_id` set to `NULL` (native, via `ON DELETE SET NULL`)

### Transactions

- `POST /transactions` — one transaction; runs the shared validation function
- `POST /transactions/batch` — `{batch_id, transactions: [...]}`; every item shares `batch_id` and is run through the same validation function; the whole batch is one SQLite transaction, so a single invalid or duplicate row rolls back the entire batch rather than partially importing it
- `GET /transactions` — full list, ordered by `date DESC, id`
- `PATCH /transactions/{id}` — `{description?, category_id?}` only. `amount`, `date`, `account_id`, `opposing_account_id`, `currency_id`, `opposing_currency_id`, `batch_id`, and `id` are immutable; a request naming any of them is a `400`, not a silent no-op. Sets `updated_at` to now.
- `DELETE /transactions/{id}` — removes the row outright (distinct from editing a structural field, so it doesn't conflict with the immutability rule above)

## Implementation

- **Crates**: `tiny_http` + `rusqlite` (`bundled` feature — compiles SQLite in, no system dependency). `axum`/`tokio` were considered and rejected: async buys concurrent request scheduling, and a single-user, single-connection workload has no concurrency to schedule, so the runtime is weight bought for a benefit that can't occur here.
- **Concurrency**: run `tiny_http`'s accept loop single-threaded (accept a request, handle it, respond, repeat). One `Connection`, plain ownership, no `Mutex` — a lock only earns its keep once a second thread can reach the same connection, and nothing here spawns one.
- **Config**: two CLI flags, `--bind` (default `127.0.0.1:8080`) and `--db` (default `./pennywise.db`). No config file, no env vars, until a third knob is needed. There is no authentication anywhere in this spec — see [ADR-0001](adr/0001-no-auth-localhost-only.md) for why the default matters and what changing `--bind` actually exposes.
