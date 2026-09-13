#!/usr/bin/env bash
# Verification harness for the v2 "Envelope" schema (wayfinder ticket #26).
# Pins the behavior decided in ADR 0004/0005 as runnable checks, so a real
# implementation has a spec to build against and a regression net to refactor under.
#
# Usage: ./verify_envelopes.sh   (exits 0 and prints all PASS, or exits 1 on any FAIL)
set -euo pipefail

DB="$(mktemp)"
trap 'rm -f "$DB"' EXIT

sqlite3 "$DB" <<'SCHEMA'
PRAGMA foreign_keys = ON;

CREATE TABLE currencies (id INTEGER PRIMARY KEY, code TEXT NOT NULL UNIQUE, minor_unit INTEGER NOT NULL);
INSERT INTO currencies VALUES (1, 'SEK', 2), (2, 'EUR', 2), (3, 'THB', 2);

CREATE TABLE accounts (
    id INTEGER PRIMARY KEY, name TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('own','external')),
    currency_id INTEGER REFERENCES currencies(id),
    CHECK ((type = 'own' AND currency_id IS NOT NULL) OR (type = 'external' AND currency_id IS NULL))
);
-- Sentinel: reserved at id 0, seeded before any user-created account can claim it.
INSERT INTO accounts VALUES (0, 'Envelope', 'external', NULL);
-- Scenario 1 (fund/spend/overspend one envelope):
INSERT INTO accounts VALUES (1, 'Checking', 'own', 1);
INSERT INTO accounts VALUES (3, 'Grocery Store', 'external', NULL);
-- Scenario 2 (credit-card lifecycle, isolated accounts so it can't interfere with scenario 1):
INSERT INTO accounts VALUES (10, 'Checking2', 'own', 1);
INSERT INTO accounts VALUES (11, 'Savings2', 'own', 1);
INSERT INTO accounts VALUES (12, 'Employer2', 'external', NULL);
INSERT INTO accounts VALUES (13, 'CreditCard2', 'own', 1);
INSERT INTO accounts VALUES (14, 'Hotel2', 'external', NULL);
-- Scenario 3 (multi-currency spanning):
INSERT INTO accounts VALUES (20, 'EUR wallet', 'own', 2);
INSERT INTO accounts VALUES (21, 'THB wallet', 'own', 3);

CREATE TABLE categories (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
-- ADR 0005: no ON DELETE clause -> defaults to RESTRICT (NO ACTION), reversing v1's SET NULL.
CREATE TABLE envelopes (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE);
INSERT INTO envelopes VALUES (1, 'Groceries'), (2, 'Vacation');
INSERT INTO categories VALUES (1, 'Food');

CREATE TABLE transactions (
    id TEXT PRIMARY KEY, description TEXT NOT NULL, date TEXT NOT NULL,
    amount INTEGER NOT NULL CHECK (amount != 0),
    opposing_amount INTEGER CHECK (opposing_amount IS NULL OR opposing_amount > 0),
    account_id INTEGER NOT NULL REFERENCES accounts(id),
    opposing_account_id INTEGER NOT NULL REFERENCES accounts(id),
    category_id INTEGER REFERENCES categories(id),          -- ADR 0005: RESTRICT (was SET NULL in v1)
    envelope_id INTEGER REFERENCES envelopes(id),            -- ADR 0005: RESTRICT
    is_funding GENERATED ALWAYS AS (opposing_account_id = 0) VIRTUAL,
    CHECK (account_id <> opposing_account_id)
);

-- raw double-entry rows, both sides, every Transaction including funding
CREATE VIEW postings AS
SELECT account_id, amount, envelope_id, is_funding FROM transactions
UNION ALL
SELECT opposing_account_id,
       CASE WHEN opposing_amount IS NOT NULL THEN -SIGN(amount)*ABS(opposing_amount) ELSE -amount END,
       envelope_id, is_funding
FROM transactions;

-- ADR 0004: GET /ledger (renamed from GET /postings) - real money only, funding excluded
CREATE VIEW ledger AS SELECT * FROM postings WHERE NOT is_funding;

-- Funded minus spent, per real Account per Envelope. Sign flipped on funding rows
-- (envelope's perspective, not the real account's) and restricted to 'own' accounts
-- so external accounts (sentinel + merchants) never leak a bogus balance row.
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
SCHEMA

# --- Scenario 1: fund, spend, overspend one envelope; category_id referenced for the RESTRICT check --
sqlite3 "$DB" <<'EOF'
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, category_id, envelope_id)
VALUES ('s1t0', 'Opening balance', '2026-01-01', 1000, 1, 3, NULL, NULL);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s1t1', 'Fund groceries', '2026-01-02', -500, 1, 0, 1);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, category_id, envelope_id)
VALUES ('s1t2', 'Weekly shop', '2026-01-03', -300, 1, 3, 1, 1);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s1t3', 'Overspend groceries', '2026-01-04', -250, 1, 3, 1);
EOF

# --- Scenario 2: fund in one account, spend on a different (credit card) account, reallocate, pay off --
sqlite3 "$DB" <<'EOF'
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s2t0', 'Salary', '2026-01-01', 2000, 10, 12, NULL);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s2t1', 'Move to savings, no tag', '2026-01-02', -700, 10, 11, NULL);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s2t2', 'Fund vacation', '2026-01-03', -400, 11, 0, 2);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s2t3', 'Hotel on credit card', '2026-01-04', -400, 13, 14, 2);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s2t4', 'Move vacation funds to checking', '2026-01-05', -400, 11, 10, 2);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s2t5', 'Pay off credit card', '2026-01-06', -400, 10, 13, 2);
EOF

# --- Scenario 3: one envelope funded from accounts of three different currencies -----------------
sqlite3 "$DB" <<'EOF'
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s3t0', 'Fund vacation EUR', '2026-01-10', -10000, 20, 0, 2);
INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id, envelope_id)
VALUES ('s3t1', 'Fund vacation THB', '2026-01-11', -500000, 21, 0, 2);
EOF

echo "=== Assertions ==="
FAIL=0
check() {
  local name="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    echo "PASS  $name"
  else
    echo "FAIL  $name (expected $expected, got $actual)"
    FAIL=1
  fi
}
q() { sqlite3 "$DB" "$1"; }

# Scenario 1
check "s1: Groceries envelope balance = funded(500) - spent(300+250) = -50" \
  "-50" "$(q "SELECT balance FROM envelope_balances WHERE account_id=1 AND envelope_id=1")"
check "s1: no external-account leakage into envelope_balances (Grocery Store, id 3)" \
  "0" "$(q "SELECT COUNT(*) FROM envelope_balances WHERE account_id=3")"
check "s1: Checking real balance (GET /ledger) excludes the funding row" \
  "450" "$(q "SELECT SUM(amount) FROM ledger WHERE account_id=1")"
check "s1: overspent envelope reserves 0, not negative (account_headroom)" \
  "0" "$(q "SELECT still_reserved FROM account_headroom WHERE account_id=1")"
check "s1: headroom == real balance once Groceries is overspent (nothing left reserved)" \
  "450" "$(q "SELECT headroom FROM account_headroom WHERE account_id=1")"

# Scenario 2 (credit-card lifecycle, fully settled)
check "s2: after full payoff, Vacation balance is 0 on every account it ever touched" \
  "0" "$(q "SELECT COUNT(*) FROM envelope_balances WHERE envelope_id=2 AND account_id IN (10,11,13) AND balance != 0")"
check "s2: after payoff, Credit Card real balance is back to 0" \
  "0" "$(q "SELECT SUM(amount) FROM ledger WHERE account_id=13")"
check "s2: Checking2 real balance nets correctly across the whole lifecycle" \
  "1300" "$(q "SELECT SUM(amount) FROM ledger WHERE account_id=10")"
check "s2: Checking2 headroom == real balance once Vacation is fully settled" \
  "1300" "$(q "SELECT headroom FROM account_headroom WHERE account_id=10")"
check "s2: Savings2 real balance reflects money passed through, not retained" \
  "300" "$(q "SELECT SUM(amount) FROM ledger WHERE account_id=11")"

# Scenario 3 (multi-currency spanning - one envelope, three currencies, never summed)
check "s3: Vacation funded per-account, EUR wallet shows its own currency's balance" \
  "10000" "$(q "SELECT balance FROM envelope_balances WHERE account_id=20 AND envelope_id=2")"
check "s3: Vacation funded per-account, THB wallet shows its own currency's balance" \
  "500000" "$(q "SELECT balance FROM envelope_balances WHERE account_id=21 AND envelope_id=2")"

echo
echo "=== ON DELETE RESTRICT checks (ADR 0005) ==="
if sqlite3 "$DB" "PRAGMA foreign_keys = ON; DELETE FROM envelopes WHERE id = 1;" 2>/dev/null; then
  echo "FAIL  deleting a referenced Envelope should be rejected, but it succeeded"
  FAIL=1
else
  echo "PASS  deleting a referenced Envelope is rejected (FK RESTRICT)"
fi
if sqlite3 "$DB" "PRAGMA foreign_keys = ON; DELETE FROM categories WHERE id = 1;" 2>/dev/null; then
  echo "FAIL  deleting a referenced Category should be rejected, but it succeeded"
  FAIL=1
else
  echo "PASS  deleting a referenced Category is rejected (FK RESTRICT, reverses v1)"
fi

echo
if [ "$FAIL" -eq 0 ]; then
  echo "ALL CHECKS PASSED"
else
  echo "SOME CHECKS FAILED"
  exit 1
fi
