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

The table shapes below are finalized. Two additions were made on top of the original design, both flagged inline: `amount`/`opposing_amount` are `INTEGER` minor units rather than `REAL` (float drift makes a ledger's core numbers untrustworthy), and `currencies` gained `minor_unit` so the file is self-describing to any consumer that has to format an amount without a hardcoded ISO 4217 table. `categories.category_id`'s `ON DELETE SET NULL` is likewise new — it's required to realize "category deletion never breaks a transaction," which has no default in SQLite.

```sql
CREATE TABLE currencies (
    id         INTEGER PRIMARY KEY,
    code       TEXT NOT NULL UNIQUE,                          -- ISO 4217, e.g. 'SEK', 'USD', 'EUR'
    minor_unit INTEGER NOT NULL CHECK (minor_unit BETWEEN 0 AND 4)  -- decimal places: 2 for USD, 0 for JPY
);

CREATE TABLE accounts (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    type        TEXT NOT NULL CHECK (type IN ('own','external')),
    currency_id INTEGER REFERENCES currencies(id),
    CHECK (
        (type = 'own'      AND currency_id IS NOT NULL) OR
        (type = 'external' AND currency_id IS NULL)
    )
);

CREATE TABLE categories (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE transactions (
    id                   TEXT PRIMARY KEY,   -- caller-supplied (e.g. hash of bank id); sole dedup mechanism
    batch_id             TEXT,               -- caller-supplied; groups an import for future bulk undo. NULL for standalone entries.
    description          TEXT NOT NULL,
    date                 TEXT NOT NULL,
    amount               INTEGER NOT NULL CHECK (amount != 0),               -- signed minor units; +ve = inflow to account_id
    opposing_amount      INTEGER CHECK (opposing_amount IS NULL OR opposing_amount > 0),  -- unsigned magnitude only
    account_id           INTEGER NOT NULL REFERENCES accounts(id),
    opposing_account_id  INTEGER NOT NULL REFERENCES accounts(id),
    category_id          INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    created_at           TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at           TEXT NOT NULL DEFAULT (datetime('now')),
    CHECK (account_id <> opposing_account_id),
    CHECK (substr(date,1,19) IS strftime('%Y-%m-%dT%H:%M:%S', substr(date,1,19)))
);

CREATE UNIQUE INDEX accounts_name_type ON accounts (name, type);
```

The two `transactions` `CHECK`s are a database-level backstop for constraints already enforced at the API layer (`validate_transaction`'s account check, and `is_rfc3339`'s full-RFC3339 date check) — they exist to catch writes that don't go through the app (a manual edit, a future script), not to replace the app-level checks. The date `CHECK` validates only the `YYYY-MM-DDTHH:MM:SS` prefix via `strftime` (real calendar correctness — rejects month 13, hour 25, unpadded digits); it uses `IS` rather than `=` because `strftime` returns `NULL` on unparseable input and a `NULL`-evaluating `CHECK` otherwise passes. It does not validate the trailing timezone offset/`Z` — SQLite has no native offset-aware date parsing, so that half of RFC3339 stays an app-only guarantee.

`account_id`/`opposing_account_id` references need no explicit `ON DELETE` clause: SQLite's default (`NO ACTION`, enforced immediately since nothing here defers foreign keys) already rejects deleting an account that transactions still point at, which is the desired behavior.

### Connection setup

Every connection opens with:

```sql
PRAGMA foreign_keys = ON;
```

SQLite does not enforce `REFERENCES` clauses without this — off by default, it would make every FK above decorative.

### Migrations

On startup, read `PRAGMA user_version`. If `0`, run the embedded schema above once (already including both `transactions` `CHECK`s) and set it to `2`. If `1` — a database created before those `CHECK`s existed — rebuild `transactions` (SQLite can't `ALTER TABLE ... ADD CHECK`: create a new table with the constraints, copy rows across, drop the old one, rename) and set it to `2`. If `2`, do nothing. Any other value panics — an unrecognized schema version means the database is newer than this binary, or corrupt, and there is nothing safe to do but stop. No migration framework — the schema is finalized, so v1 has exactly two migrations; add a framework at migration three, not migration one.

## Cross-table validation

Not expressible as SQLite `CHECK` constraints (they span rows/tables). Implement as **one shared function** that every insert/update path calls — never duplicate these checks per-endpoint:

1. `account_id` must reference an account with `type = 'own'`.
2. `opposing_amount` is required if and only if both `account_id` and `opposing_account_id` are `type = 'own'` and their currencies differ. It is otherwise `NULL`. When present, it is the magnitude on the opposing side only — its sign is always inferred from `amount`'s sign, never stored separately.

The relationship between `amount` and `opposing_amount` on a cross-currency Transfer — i.e. the implied exchange rate — is deliberately unvalidated. Nothing checks it's a plausible rate; a scale error (e.g. an import feeding major units where minor units were expected) is accepted same as a correct pair. No information is lost (the pair is always recoverable), and there is no rate source in this project to validate against (multi-currency net worth/exchange-rate fetching is an explicit non-goal). The owner is responsible for the pair being sane, same trust level as `description`.

Everything else (an account/category id existing at all, an `own` account having a currency, an `external` account not having one) is already enforced natively by the schema's `CHECK`/`REFERENCES` clauses plus `PRAGMA foreign_keys = ON` — don't re-implement it in application code.

## API

HTTP + JSON. No filtering, no pagination, no aggregation on any endpoint — a list endpoint returns everything; a consumer that needs a subset or a sum computes it itself.

Standard envelope: `{"error": "<message>"}` on failure. Status codes: `201` create, `200` read, `204` delete, `400` validation failure (including a `CHECK`/`FK` violation surfaced from SQLite, and an attempt to `PATCH` an immutable field), `404` unknown id, `409` duplicate `id` on insert (primary-key conflict — this **is** the dedup mechanism; don't build a second one).

The endpoint inventory (paths, request/response shapes, per-endpoint status codes) lives in `docs/openapi.yaml`, the machine-readable source of truth — don't restate it here. Update it in the same commit as any change to an endpoint's shape or behavior.

## Implementation

- **Crates**: `tiny_http` + `rusqlite` (`bundled` feature — compiles SQLite in, no system dependency). `axum`/`tokio` were considered and rejected: async buys concurrent request scheduling, and a single-user, single-connection workload has no concurrency to schedule, so the runtime is weight bought for a benefit that can't occur here.
- **Concurrency**: run `tiny_http`'s accept loop single-threaded (accept a request, handle it, respond, repeat). One `Connection`, plain ownership, no `Mutex` — a lock only earns its keep once a second thread can reach the same connection, and nothing here spawns one.
- **Config**: two CLI flags, `--bind` (default `127.0.0.1:8080`) and `--db` (default `./pennywise.db`). No config file, no env vars, until a third knob is needed. There is no authentication anywhere in this spec — see [ADR-0001](adr/0001-no-auth-localhost-only.md) for why the default matters and what changing `--bind` actually exposes.
