# `category_id` and `envelope_id` deletes RESTRICT, not SET NULL

v1 gave `category_id` `ON DELETE SET NULL`, reasoned explicitly in `docs/spec-v1.md`: Category is "purely descriptive — nothing in the Ledger depends on it being set or correct" (`CONTEXT.md`), so silently detaching it on delete costs nothing real.

Envelope breaks that reasoning. Unlike Category, an Envelope's balance is a real computed financial signal — `account_headroom` depends on it. If `envelope_id` followed the same `SET NULL` pattern, deleting an Envelope that still holds a funded-but-unspent balance would silently increase every affected Account's reported headroom, with no Transaction to justify the change. That's not a lost label, it's a balance-integrity bug.

Rather than give `envelope_id` different delete behavior than `category_id` for what's ultimately the same underlying question ("can a Transaction end up silently detached from something it referenced"), both now get the same answer: neither carries an explicit `ON DELETE` clause, so both fall back to SQLite's default (`NO ACTION`, enforced immediately) — the same behavior `accounts` already relies on, which `docs/spec-v1.md` itself calls "the desired behavior" for exactly this reason. This reverses v1's `category_id` `SET NULL` choice.

Three options were weighed for `envelope_id`:

- **(a) Match `accounts`**: implicit `RESTRICT` — chosen.
- **(b) Match v1's `categories`**: `ON DELETE SET NULL` — rejected, reintroduces the silent-headroom-drift risk above.
- **(c) An app-level guard**: reject the delete with `400` unless the Envelope's balance is `0` everywhere, `RESTRICT` as a DB-level backstop — punted. It's more usable (lets a fully-spent Envelope actually be deleted) but is new application code for a need that hasn't shown up yet; nothing stops adding it later without a schema change, since it would sit in front of a `RESTRICT` that already exists.

## Consequences

- A Category or Envelope referenced by any Transaction can never be deleted in v2, full stop — a real usability gap (no way to retire a stale one without deleting its history) traded deliberately for zero silent data loss.
- This is a breaking change to already-shipped v1 behavior for `category_id`, carried out in v2 rather than left as v1's decision to revisit — same category as the `batch_id` removal ([ADR 0003](0003-drop-batch-id.md)).
- If the inability to delete a fully-spent Envelope or an unused Category becomes a real complaint, option (c) above is the documented upgrade path — revisit then, with a real complaint to design against instead of a hypothetical one (same posture as [ADR 0002](0002-currency-stays-on-account.md)'s deferral).
