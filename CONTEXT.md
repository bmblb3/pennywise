# Pennywise

Pennywise is a personal finance ledger for one person's own money: a record of Transactions between Accounts.

## Accounts

**Account**:
A place money sits or is owed.
_Avoid_: wallet, source, pot, user account

**Own Account**:
An Account the ledger's owner controls — a bank account, a savings pot, a credit card, cash in a drawer. Not tied to a single Currency: cash in a drawer or a Wise-style account may hold more than one at once.
_Avoid_: internal account, my account

**External Account**:
An Account outside the ledger owner's control — a shop, an employer, another person.
_Avoid_: payee, merchant, vendor, counterparty (as a noun on its own)

**Currency**:
The unit a Transaction's Amount is denominated in, identified by its ISO 4217 code. A property of the Transaction, not the Account — neither an Own nor an External Account has a Currency of its own.
_Avoid_: denomination

## The ledger

**Ledger**:
The complete record of Transactions.
_Avoid_: history, journal, book, log

**Transaction**:
A single dated movement of money from one Account to another.
_Avoid_: entry, record, payment, purchase, expense

**Amount**:
The signed sum a Transaction moves, relative to its Account: positive means money arrived there, negative means it left. Expressed as a whole count of its Currency's smallest unit (e.g. cents), never a fraction.
_Avoid_: value, sum, price, cost

**Opposing Account**:
The Account on the other side of a Transaction from its Account. An Own Account paying an External Account, or two Own Accounts moving money between each other, are both expressed the same way: one Account, one Opposing Account, one signed Amount.
_Avoid_: counterparty, other side, destination account

**Opposing Currency**:
The Currency of the Opposing Account's side of a Transaction, recorded only when it differs from the Transaction's own Currency — a currency conversion, or a foreign-currency purchase at an External Account.
_Avoid_: destination currency, foreign currency

**Expense** / **Income**:
The direction of a Transaction's Amount — money leaving an Own Account, or arriving at one. A direction, never a synonym for the Transaction itself.
_Avoid_: debit, credit, outgoing, spend, earnings

**Transfer**:
A Transaction where both the Account and the Opposing Account are Own Accounts. It moves money between the owner's own Accounts rather than to or from the outside world.
_Avoid_: internal payment, self-payment

**Category**:
The classification attached to a Transaction for the owner's own reference. Purely descriptive — nothing in the Ledger depends on it being set or correct.
_Avoid_: tag, label, bucket, type

**Batch**:
A group of Transactions inserted together, usually from one import, sharing a single identifier so they can be removed as a unit.
_Avoid_: import, upload, group
