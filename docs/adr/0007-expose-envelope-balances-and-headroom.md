# Expose `GET /envelope_balances` and `GET /account_headroom`

ADR 0004 introduced `envelope_balances` and `account_headroom` purely as schema-level devices: SQL views that pin the funding mechanism's correctness and are exercised by its `.verify.sh` harness, deliberately *not* exposed through any endpoint. spec-v2.md was explicit about this — "Neither `envelope_balances` nor `account_headroom` is exposed via any endpoint in this spec... a client wanting an Envelope's standing today computes it from `GET /ledger`/`GET /transactions` itself" — consistent with the stance a few lines up that `GET /balances` is "the only aggregation endpoint in v2... net worth, category summaries, and per-period totals stay out."

[ADR 0006](0006-dedicated-fundings-endpoint.md) already narrowed the adjacent "everything is a Transaction, no special-cased path" stance for write ergonomics. The same pressure showed up on the read side once `POST /fundings` existed: an Envelope's balance and an Account's headroom are exactly the numbers that mechanism exists to produce, and forcing every client to re-derive them via a `GET /fundings` + `GET /ledger` join (or, before ADR 0006, a raw `GET /transactions` scan) is work this API already does internally to define what "correct" even means for funding — it was already computed, just not exposed.

## Decision

Add `GET /envelope_balances` and `GET /account_headroom`, named directly after the SQL views ADR 0004 already defined — no new vocabulary. Both are computed in application code, not literal SQL views (the same choice already made for `ledger`/`balances`; spec-v2.md's note that "`postings` as a SQL view is a modeling convenience... not a commitment to a literal SQL object" applies here too):

- **`GET /envelope_balances`** — one row per `(account, envelope)` that has at least one Transaction tagged with that Envelope: `{account, envelope, currency, balance}`, denormalized names like `GET /ledger`. `balance` can be negative (overspent).
- **`GET /account_headroom`** — one row per Own Account, including ones with no Envelope activity: `{account_id, account_name, currency, bal, still_reserved, headroom}`. `bal` is the same number `GET /balances` reports; `still_reserved` sums each Envelope's balance on that Account floored at `0`; `headroom` is `bal - still_reserved` and can go negative.

## Consequences

- **This narrows, not reopens, the "only aggregation endpoint in v2" line.** `GET /balances` remains the only endpoint aggregating real money movement; these two are a separate, narrowly-scoped pair over the Envelope mechanism specifically. Net worth, category summaries, and per-period totals are still out of scope — nothing about this decision generalizes past what ADR 0004 already pinned in the schema.
- **Overspend semantics are unchanged**, just now readable directly: an overspent Envelope's negative `envelope_balances.balance` contributes `0` (never negative) to `account_headroom.still_reserved`, and `headroom` is a signal, not an enforced limit — funding beyond what's really there is still legal and just shows up as negative headroom.
- **`GET /transactions`/`GET /ledger` + `GET /fundings` joins still work** for a client that wants the raw legs instead of the aggregate — this doesn't replace them, it adds the number a client would otherwise have had to derive.
