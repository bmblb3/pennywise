# Pennywise

Pennywise is a personal finance ledger for one person's own money: a record of Transactions between Accounts.

## Accounts

**Account**:
A place money sits or is owed.
_Avoid_: wallet, source, pot, user account

**Own Account**:
An Account the ledger's owner controls — a bank account, a savings pot, a credit card, cash in a drawer. Denominated in a single Currency.
_Avoid_: internal account, my account

**External Account**:
An Account outside the ledger owner's control — a shop, an employer, another person. Takes on whatever Currency a Transaction against it used, since it isn't the owner's to denominate.
_Avoid_: payee, merchant, vendor, counterparty (as a noun on its own)

**Currency**:
The unit an Own Account's Amounts are denominated in, identified by its ISO 4217 code.
_Avoid_: denomination

## The ledger

**Ledger**:
The record of real-money Transactions — every Transaction except Envelope funding, which reserves money in place without moving it. Exposed via `GET /ledger`.
_Avoid_: history, journal, book, log

**Transaction**:
A single dated movement of money from one Account to another. Deletion is not durable: a Transaction's `id` is the sole dedup mechanism (a re-insert with the same `id` is rejected), and a hard delete frees that `id`, so re-importing the same source file brings a deliberately deleted Transaction back. This is accepted, not a bug — the owner rarely deletes, and a delete is usually a mistake anyway.
_Avoid_: entry, record, payment, purchase, expense

**Amount**:
The signed sum a Transaction moves, relative to its Account: positive means money arrived there, negative means it left. Expressed as a whole count of the Currency's smallest unit (e.g. cents), never a fraction.
_Avoid_: value, sum, price, cost

**Opposing Account**:
The Account on the other side of a Transaction from its Account. An Own Account paying an External Account, or two Own Accounts moving money between each other, are both expressed the same way: one Account, one Opposing Account, one signed Amount.
_Avoid_: counterparty, other side, destination account

**Expense** / **Income**:
The direction of a Transaction's Amount — money leaving an Own Account, or arriving at one. A direction, never a synonym for the Transaction itself.
_Avoid_: debit, credit, outgoing, spend, earnings

**Transfer**:
A Transaction where both the Account and the Opposing Account are Own Accounts. It moves money between the owner's own Accounts rather than to or from the outside world.
_Avoid_: internal payment, self-payment

**Category**:
The classification attached to a Transaction for the owner's own reference. Purely descriptive — nothing in the Ledger depends on it being set or correct.
_Avoid_: tag, label, bucket, type

**Envelope**:
A named, virtual reservation of money already sitting in one or more Own Accounts — never a place money moves to. Funding an Envelope is a Transaction whose Opposing Account is the reserved sentinel External Account (id `0`, name "Envelope"); it reserves money in the Transaction's own Account without moving it, so it never appears in the Ledger. An Envelope's balance (funded minus spent) is computed per Own Account touched, never summed across Accounts — a reservation can span Accounts of different Currencies. Can be negative if overspent; nothing enforces non-negative balances. Deleting an Envelope any Transaction still references is rejected, same as an Account. Exposed via `GET /envelope_balances`; funding one is `POST /fundings`.
_Avoid_: piggy bank, budget, pot, bucket, goal

**Headroom**:
How much of an Own Account's real balance isn't already reserved by its Envelopes: the Account's balance minus the sum of its Envelopes' balances there, each floored at `0` (an overspent Envelope can't reserve money it doesn't have). A signal, not an enforced limit — funding an Envelope from an Account that doesn't have the money to spare is legal, and headroom just goes negative. Exposed via `GET /account_headroom`.
_Avoid_: available balance, spendable, free balance
