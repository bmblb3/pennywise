# Drop `batch_id`, retire the Batch concept

v1 shipped `batch_id` on every Transaction for exactly one stated reason: "future bulk undo" (`spec-v1.md`'s Non-goals list named the undo endpoint, `DELETE /batches/{id}`, as the deferred consumer). v2 charting picked that endpoint up and spec'd it (closed ticket [Spec: DELETE /batches/{id}](https://github.com/bmblb3/pennywise/issues/25)) — then, before implementation, the owner reconsidered: bulk-undo-by-id was a complication invented to justify a column that had never done anything, not a need. `DELETE /batches/{id}` is scrapped, and with no other consumer of `batch_id`, so is the column: it drops via migration, and the field disappears from `POST /transactions` and `POST /transactions/batch`. This is a breaking change to already-shipped v1 API, carried out in v2 rather than left as a decision v1 hasn't made yet.

`CONTEXT.md`'s **Batch** term is retired along with it — it was defined entirely by the thing being removed ("a group of Transactions... sharing a single identifier so they can be removed as a unit"), and no other definition of it was ever in use. `POST /transactions/batch` keeps its name and shape (minus the `batch_id` field): it's still a genuinely atomic multi-row insert, rolled back together on any invalid or duplicate row, just no longer naming a domain concept — "batch" there is now plain English, not a capitalized term.

## Consequences

Existing rows' `batch_id` values are dropped with the column, unexported. This loses zero live capability: no endpoint ever read that grouping back out (`GET /batches` was never built, and the v2 map ruled it out of scope on charting), so nothing regresses beyond ceasing to store data nothing consumed.
