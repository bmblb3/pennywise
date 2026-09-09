# Pennywise

## Agent skills

### Issue tracker

Issues live as GitHub issues on `bmblb3/pennywise`, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical roles, using the default label strings (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `CONTEXT.md` and one `docs/adr/` at the repo root, both created lazily by `/domain-modeling`. See `docs/agents/domain.md`.

### Docs stay in sync

When a change alters an endpoint's request/response shape or behavior, update `docs/openapi.yaml` in the same commit. If it also changes the storage contract or a cross-table invariant, update `docs/spec-v1.md` too. Don't leave doc updates for a follow-up.

### Coding posture

**`/ponytail` at `full` intensity is the standing default for this repo.** Prefer the laziest solution that works: question whether the task needs to exist (YAGNI), reach for the standard library before custom code and native platform features before dependencies, one line before fifty. Mark deliberate shortcuts with a `ponytail:` comment so `/ponytail-debt` can harvest them into a ledger. Related: `/ponytail-review` (over-engineering review of a diff), `/ponytail-audit` (whole-repo scan).
