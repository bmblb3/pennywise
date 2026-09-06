use rusqlite::Connection;
use std::io::Cursor;
use tiny_http::{Header, Method, Response, Server};

const SCHEMA: &str = "
CREATE TABLE currencies (
    id         INTEGER PRIMARY KEY,
    code       TEXT NOT NULL UNIQUE,
    minor_unit INTEGER NOT NULL CHECK (minor_unit BETWEEN 0 AND 4)
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
    id                   TEXT PRIMARY KEY,
    batch_id             TEXT,
    description          TEXT NOT NULL,
    date                 TEXT NOT NULL,
    amount               INTEGER NOT NULL CHECK (amount != 0),
    opposing_amount      INTEGER CHECK (opposing_amount IS NULL OR opposing_amount > 0),
    account_id           INTEGER NOT NULL REFERENCES accounts(id),
    opposing_account_id  INTEGER NOT NULL REFERENCES accounts(id),
    category_id          INTEGER REFERENCES categories(id) ON DELETE SET NULL,
    created_at           TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at           TEXT NOT NULL DEFAULT (datetime('now'))
);
";

struct Config {
    bind: String,
    db: String,
}

impl Config {
    fn from_args() -> Self {
        let mut bind = "127.0.0.1:8080".to_string();
        let mut db = "./pennywise.db".to_string();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--bind" => bind = args.next().expect("--bind requires a value"),
                "--db" => db = args.next().expect("--db requires a value"),
                other => panic!("unknown argument: {other}"),
            }
        }
        Config { bind, db }
    }
}

fn open_db(path: &str) -> Connection {
    let conn = Connection::open(path).expect("failed to open database");
    conn.execute_batch("PRAGMA foreign_keys = ON;")
        .expect("failed to enable foreign keys");
    conn
}

fn migrate(conn: &Connection) {
    let user_version: i64 = conn
        .query_row("PRAGMA user_version;", [], |row| row.get(0))
        .expect("failed to read user_version");
    match user_version {
        0 => {
            conn.execute_batch(SCHEMA)
                .expect("failed to apply schema");
            conn.execute_batch("PRAGMA user_version = 1;")
                .expect("failed to set user_version");
        }
        1 => {}
        other => panic!("unknown database schema version: {other}"),
    }
}

/// The one shared cross-table validation function every transaction insert/update
/// path must call (see docs/spec-v1.md "Cross-table validation"). Everything else
/// (id existence, own-account-has-currency, external-account-has-no-currency) is
/// already enforced by the schema's CHECK/REFERENCES + PRAGMA foreign_keys = ON.
fn validate_transaction(
    conn: &Connection,
    account_id: i64,
    opposing_account_id: i64,
    opposing_amount: Option<i64>,
) -> Result<(), String> {
    let lookup = |id: i64| -> Result<(String, Option<i64>), String> {
        conn.query_row(
            "SELECT type, currency_id FROM accounts WHERE id = ?1",
            [id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .map_err(|_| format!("account {id} does not exist"))
    };

    let (account_type, account_currency) = lookup(account_id)?;
    if account_type != "own" {
        return Err(format!("account {account_id} must be of type 'own'"));
    }

    let (opposing_type, opposing_currency) = lookup(opposing_account_id)?;
    let needs_opposing_amount = opposing_type == "own" && opposing_currency != account_currency;

    match (needs_opposing_amount, opposing_amount) {
        (true, None) => Err("opposing_amount is required when both accounts are 'own' with differing currencies".to_string()),
        (false, Some(_)) => Err("opposing_amount must be NULL unless both accounts are 'own' with differing currencies".to_string()),
        _ => Ok(()),
    }
}

// --- HTTP response helpers -------------------------------------------------
//
// Every endpoint (added in later tickets) replies through these so the
// envelope and status codes stay consistent: 201 create, 200 read, 204
// delete, 400 validation failure, 404 unknown id, 409 duplicate id on insert.

fn json_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn json_response(status: u16, body: String) -> Response<Cursor<Vec<u8>>> {
    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    Response::from_string(body)
        .with_status_code(status)
        .with_header(header)
}

/// `{"error": "<message>"}` envelope for any failure response.
fn error_response(status: u16, message: &str) -> Response<Cursor<Vec<u8>>> {
    json_response(status, format!(r#"{{"error": "{}"}}"#, json_escape(message)))
}

fn not_found() -> Response<Cursor<Vec<u8>>> {
    error_response(404, "not found")
}

/// Maps a `rusqlite::Error` to the (status, message) an endpoint should
/// respond with: a primary-key/unique conflict on insert is `409`, any other
/// `CHECK`/`FK` constraint failure is `400`.
fn map_db_error(err: &rusqlite::Error) -> (u16, String) {
    if let rusqlite::Error::SqliteFailure(sqlite_err, message) = err {
        let is_duplicate_id = matches!(
            sqlite_err.extended_code,
            rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY | rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        );
        if is_duplicate_id {
            return (409, "duplicate id".to_string());
        }
        if sqlite_err.code == rusqlite::ErrorCode::ConstraintViolation {
            return (
                400,
                message.clone().unwrap_or_else(|| "constraint violation".to_string()),
            );
        }
    }
    (400, err.to_string())
}

fn db_error_response(err: &rusqlite::Error) -> Response<Cursor<Vec<u8>>> {
    let (status, message) = map_db_error(err);
    error_response(status, &message)
}

// --- Minimal hand-rolled JSON -------------------------------------------
//
// Request/response bodies in this API are small, flat, fixed-shape objects
// (a handful of string/int fields, or a list of such objects) — not
// arbitrary JSON. A generic parser/serializer (or a serde dependency) would
// be more code and more risk than reading the couple of fields each
// endpoint actually needs. See /ponytail posture in CLAUDE.md.

/// Extracts a top-level `"key": "value"` string field from a flat JSON
/// object. No nesting, no unicode escapes — just enough for this API's
/// request bodies.
fn json_str_field(body: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = &body[body.find(&needle)? + needle.len()..];
    let after_colon = after_key[after_key.find(':')? + 1..].trim_start();
    let rest = after_colon.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// Extracts a top-level `"key": <integer>` field from a flat JSON object.
fn json_int_field(body: &str, key: &str) -> Option<i64> {
    let needle = format!("\"{key}\"");
    let after_key = &body[body.find(&needle)? + needle.len()..];
    let after_colon = after_key[after_key.find(':')? + 1..].trim_start();
    let end = after_colon
        .find(|c: char| !(c.is_ascii_digit() || c == '-'))
        .unwrap_or(after_colon.len());
    after_colon[..end].parse().ok()
}

fn json_string(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

/// Splits out each top-level `{...}` object from a string, skipping over
/// quoted strings so braces/brackets inside a description don't confuse the
/// brace count. Used to pull the repeated transaction objects out of a
/// `"transactions": [...]` array — the only nesting this API's request
/// bodies need.
fn split_json_objects(s: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    objects.push(s[start..=i].to_string());
                }
            }
            _ => {}
        }
    }
    objects
}

/// Extracts a top-level `"key": [ {...}, {...} ]` field as the list of raw
/// object bodies. No index into the array is needed by callers, so this
/// skips finding the closing `]` and just collects every balanced `{...}`
/// after the key — cheaper than tracking the array bound separately.
fn json_object_array_field(body: &str, key: &str) -> Option<Vec<String>> {
    let needle = format!("\"{key}\"");
    let after_key = &body[body.find(&needle)? + needle.len()..];
    let after_colon = &after_key[after_key.find(':')? + 1..];
    let objects = split_json_objects(after_colon);
    if objects.is_empty() {
        None
    } else {
        Some(objects)
    }
}

// --- Currencies ---------------------------------------------------------

fn currency_json(row: &rusqlite::Row) -> rusqlite::Result<String> {
    let id: i64 = row.get(0)?;
    let code: String = row.get(1)?;
    let minor_unit: i64 = row.get(2)?;
    Ok(format!(
        r#"{{"id": {id}, "code": {}, "minor_unit": {minor_unit}}}"#,
        json_string(&code)
    ))
}

fn create_currency(conn: &Connection, body: &str) -> Response<Cursor<Vec<u8>>> {
    let (Some(code), Some(minor_unit)) =
        (json_str_field(body, "code"), json_int_field(body, "minor_unit"))
    else {
        return error_response(400, "expected {code, minor_unit}");
    };
    match conn.query_row(
        "INSERT INTO currencies (code, minor_unit) VALUES (?1, ?2) RETURNING id, code, minor_unit",
        rusqlite::params![code, minor_unit],
        currency_json,
    ) {
        Ok(json) => json_response(201, json),
        Err(err) => db_error_response(&err),
    }
}

fn list_currencies(conn: &Connection) -> Response<Cursor<Vec<u8>>> {
    let mut stmt = conn
        .prepare("SELECT id, code, minor_unit FROM currencies")
        .expect("prepare list currencies");
    let rows: Vec<String> = stmt
        .query_map([], currency_json)
        .expect("query currencies")
        .map(|r| r.expect("read currency row"))
        .collect();
    json_response(200, format!("[{}]", rows.join(", ")))
}

// --- Categories -----------------------------------------------------------

fn category_json(row: &rusqlite::Row) -> rusqlite::Result<String> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    Ok(format!(r#"{{"id": {id}, "name": {}}}"#, json_string(&name)))
}

fn create_category(conn: &Connection, body: &str) -> Response<Cursor<Vec<u8>>> {
    let Some(name) = json_str_field(body, "name") else {
        return error_response(400, "expected {name}");
    };
    match conn.query_row(
        "INSERT INTO categories (name) VALUES (?1) RETURNING id, name",
        [name],
        category_json,
    ) {
        Ok(json) => json_response(201, json),
        Err(err) => db_error_response(&err),
    }
}

fn list_categories(conn: &Connection) -> Response<Cursor<Vec<u8>>> {
    let mut stmt = conn
        .prepare("SELECT id, name FROM categories")
        .expect("prepare list categories");
    let rows: Vec<String> = stmt
        .query_map([], category_json)
        .expect("query categories")
        .map(|r| r.expect("read category row"))
        .collect();
    json_response(200, format!("[{}]", rows.join(", ")))
}

fn delete_category(conn: &Connection, id: &str) -> Response<Cursor<Vec<u8>>> {
    let Ok(id) = id.parse::<i64>() else {
        return not_found();
    };
    let deleted = conn
        .execute("DELETE FROM categories WHERE id = ?1", [id])
        .expect("delete category");
    if deleted == 0 {
        not_found()
    } else {
        Response::from_string("").with_status_code(204)
    }
}

// --- Accounts handlers --------------------------------------------------

fn account_json(row: &rusqlite::Row) -> rusqlite::Result<String> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    let r#type: String = row.get(2)?;
    let currency_id: Option<i64> = row.get(3)?;
    Ok(format!(
        r#"{{"id": {id}, "name": {}, "type": {}, "currency_id": {}}}"#,
        json_string(&name),
        json_string(&r#type),
        currency_id.map(|c| c.to_string()).unwrap_or_else(|| "null".to_string()),
    ))
}

fn handle_create_account(conn: &Connection, body: &str) -> Response<Cursor<Vec<u8>>> {
    let (Some(name), Some(account_type)) =
        (json_str_field(body, "name"), json_str_field(body, "type"))
    else {
        return error_response(400, "expected {name, type}");
    };
    let currency_id = json_int_field(body, "currency_id");

    match conn.query_row(
        "INSERT INTO accounts (name, type, currency_id) VALUES (?1, ?2, ?3) RETURNING id, name, type, currency_id",
        rusqlite::params![name, account_type, currency_id],
        account_json,
    ) {
        Ok(json) => json_response(201, json),
        Err(e) => db_error_response(&e),
    }
}

fn handle_list_accounts(conn: &Connection) -> Response<Cursor<Vec<u8>>> {
    let mut stmt = conn
        .prepare("SELECT id, name, type, currency_id FROM accounts")
        .expect("failed to prepare accounts query");
    let rows: Vec<String> = stmt
        .query_map([], account_json)
        .expect("failed to query accounts")
        .map(|r| r.expect("failed to read account row"))
        .collect();
    json_response(200, format!("[{}]", rows.join(", ")))
}

fn handle_delete_account(conn: &Connection, id: &str) -> Response<Cursor<Vec<u8>>> {
    let Ok(id) = id.parse::<i64>() else {
        return not_found();
    };
    match conn.execute("DELETE FROM accounts WHERE id = ?1", [id]) {
        Ok(0) => not_found(),
        Ok(_) => Response::from_string("").with_status_code(204),
        Err(e) => db_error_response(&e),
    }
}

// --- Transactions handlers -----------------------------------------------

fn transaction_json(row: &rusqlite::Row) -> rusqlite::Result<String> {
    let id: String = row.get(0)?;
    let batch_id: Option<String> = row.get(1)?;
    let description: String = row.get(2)?;
    let date: String = row.get(3)?;
    let amount: i64 = row.get(4)?;
    let opposing_amount: Option<i64> = row.get(5)?;
    let account_id: i64 = row.get(6)?;
    let opposing_account_id: i64 = row.get(7)?;
    let category_id: Option<i64> = row.get(8)?;
    let created_at: String = row.get(9)?;
    let updated_at: String = row.get(10)?;
    Ok(format!(
        r#"{{"id": {}, "batch_id": {}, "description": {}, "date": {}, "amount": {amount}, "opposing_amount": {}, "account_id": {account_id}, "opposing_account_id": {opposing_account_id}, "category_id": {}, "created_at": {}, "updated_at": {}}}"#,
        json_string(&id),
        batch_id.map(|b| json_string(&b)).unwrap_or_else(|| "null".to_string()),
        json_string(&description),
        json_string(&date),
        opposing_amount.map(|a| a.to_string()).unwrap_or_else(|| "null".to_string()),
        category_id.map(|c| c.to_string()).unwrap_or_else(|| "null".to_string()),
        json_string(&created_at),
        json_string(&updated_at),
    ))
}

const TRANSACTION_COLUMNS: &str = "id, batch_id, description, date, amount, opposing_amount, \
    account_id, opposing_account_id, category_id, created_at, updated_at";

/// Shared by `POST /transactions` and each item of `POST /transactions/batch`.
/// `forced_batch_id` overrides any `batch_id` in `body` — every row in a
/// batch shares the request's top-level `batch_id`, regardless of what an
/// individual item says.
fn insert_transaction(
    conn: &Connection,
    body: &str,
    forced_batch_id: Option<&str>,
) -> Result<String, Response<Cursor<Vec<u8>>>> {
    let (Some(id), Some(description), Some(date), Some(amount), Some(account_id), Some(opposing_account_id)) = (
        json_str_field(body, "id"),
        json_str_field(body, "description"),
        json_str_field(body, "date"),
        json_int_field(body, "amount"),
        json_int_field(body, "account_id"),
        json_int_field(body, "opposing_account_id"),
    ) else {
        return Err(error_response(
            400,
            "expected {id, description, date, amount, account_id, opposing_account_id}",
        ));
    };
    let opposing_amount = json_int_field(body, "opposing_amount");
    let category_id = json_int_field(body, "category_id");
    let batch_id = forced_batch_id
        .map(str::to_string)
        .or_else(|| json_str_field(body, "batch_id"));

    if let Err(msg) = validate_transaction(conn, account_id, opposing_account_id, opposing_amount) {
        return Err(error_response(400, &msg));
    }

    conn.query_row(
        &format!(
            "INSERT INTO transactions (id, batch_id, description, date, amount, opposing_amount, account_id, opposing_account_id, category_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             RETURNING {TRANSACTION_COLUMNS}"
        ),
        rusqlite::params![id, batch_id, description, date, amount, opposing_amount, account_id, opposing_account_id, category_id],
        transaction_json,
    )
    .map_err(|e| db_error_response(&e))
}

fn create_transaction(conn: &Connection, body: &str) -> Response<Cursor<Vec<u8>>> {
    match insert_transaction(conn, body, None) {
        Ok(json) => json_response(201, json),
        Err(resp) => resp,
    }
}

fn create_transactions_batch(conn: &Connection, body: &str) -> Response<Cursor<Vec<u8>>> {
    let (Some(batch_id), Some(items)) = (
        json_str_field(body, "batch_id"),
        json_object_array_field(body, "transactions"),
    ) else {
        return error_response(400, "expected {batch_id, transactions: [...]}");
    };

    conn.execute_batch("BEGIN").expect("begin batch transaction");

    let mut created = Vec::new();
    for item in &items {
        match insert_transaction(conn, item, Some(&batch_id)) {
            Ok(json) => created.push(json),
            Err(resp) => {
                conn.execute_batch("ROLLBACK").expect("rollback batch transaction");
                return resp;
            }
        }
    }

    conn.execute_batch("COMMIT").expect("commit batch transaction");
    json_response(201, format!("[{}]", created.join(", ")))
}

fn list_transactions(conn: &Connection) -> Response<Cursor<Vec<u8>>> {
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {TRANSACTION_COLUMNS} FROM transactions ORDER BY date DESC, id"
        ))
        .expect("prepare list transactions");
    let rows: Vec<String> = stmt
        .query_map([], transaction_json)
        .expect("query transactions")
        .map(|r| r.expect("read transaction row"))
        .collect();
    json_response(200, format!("[{}]", rows.join(", ")))
}

/// Fields immutable after creation — naming any of these in a PATCH body is
/// a 400, not a silent no-op (see docs/spec-v1.md "API" -> Transactions).
const IMMUTABLE_TRANSACTION_FIELDS: [&str; 6] =
    ["amount", "date", "account_id", "opposing_account_id", "batch_id", "id"];

fn handle_patch_transaction(conn: &Connection, id: &str, body: &str) -> Response<Cursor<Vec<u8>>> {
    let names_immutable_field = IMMUTABLE_TRANSACTION_FIELDS
        .iter()
        .any(|f| body.contains(&format!("\"{f}\"")));
    if names_immutable_field {
        return error_response(400, "only description and category_id may be updated");
    }

    let description = json_str_field(body, "description");
    let category_id = json_int_field(body, "category_id");

    let updated = conn.execute(
        "UPDATE transactions SET description = COALESCE(?1, description), category_id = COALESCE(?2, category_id), updated_at = datetime('now') WHERE id = ?3",
        rusqlite::params![description, category_id, id],
    );
    match updated {
        Ok(0) => not_found(),
        Ok(_) => conn
            .query_row(
                &format!("SELECT {TRANSACTION_COLUMNS} FROM transactions WHERE id = ?1"),
                [id],
                transaction_json,
            )
            .map(|json| json_response(200, json))
            .unwrap_or_else(|e| db_error_response(&e)),
        Err(e) => db_error_response(&e),
    }
}

fn handle_delete_transaction(conn: &Connection, id: &str) -> Response<Cursor<Vec<u8>>> {
    match conn.execute("DELETE FROM transactions WHERE id = ?1", [id]) {
        Ok(0) => not_found(),
        Ok(_) => Response::from_string("").with_status_code(204),
        Err(e) => db_error_response(&e),
    }
}

// --- Routing ----------------------------------------------------------------
//
// Dispatch skeleton: matches method + path against the known resource
// routes.

fn route(conn: &Connection, method: &Method, path: &str, body: &str) -> Response<Cursor<Vec<u8>>> {
    let segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match (method, segments.as_slice()) {
        (Method::Post, ["currencies"]) => create_currency(conn, body),
        (Method::Get, ["currencies"]) => list_currencies(conn),

        (Method::Post, ["accounts"]) => handle_create_account(conn, body),
        (Method::Get, ["accounts"]) => handle_list_accounts(conn),
        (Method::Delete, ["accounts", id]) => handle_delete_account(conn, id),

        (Method::Post, ["categories"]) => create_category(conn, body),
        (Method::Get, ["categories"]) => list_categories(conn),
        (Method::Delete, ["categories", id]) => delete_category(conn, id),

        (Method::Post, ["transactions", "batch"]) => create_transactions_batch(conn, body),
        (Method::Post, ["transactions"]) => create_transaction(conn, body),
        (Method::Get, ["transactions"]) => list_transactions(conn),
        (Method::Patch, ["transactions", id]) => handle_patch_transaction(conn, id, body),
        (Method::Delete, ["transactions", id]) => handle_delete_transaction(conn, id),

        _ => not_found(),
    }
}

fn main() {
    let config = Config::from_args();

    let conn = open_db(&config.db);
    migrate(&conn);

    let server = Server::http(&config.bind).expect("failed to bind server");
    println!("listening on {}", config.bind);

    for mut request in server.incoming_requests() {
        let method = request.method().clone();
        let path = request.url().split('?').next().unwrap_or("").to_string();
        let mut body = String::new();
        let _ = request.as_reader().read_to_string(&mut body);
        let response = route(&conn, &method, &path, &body);
        let _ = request.respond(response);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sets up an in-memory DB with two currencies (SEK id 1, USD id 2) and four
    /// accounts: 1 = own/SEK, 2 = own/SEK, 3 = own/USD, 4 = external.
    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        migrate(&conn);
        conn.execute_batch(
            "
            INSERT INTO currencies (id, code, minor_unit) VALUES (1, 'SEK', 2), (2, 'USD', 2);
            INSERT INTO accounts (id, name, type, currency_id) VALUES
                (1, 'Own SEK A', 'own', 1),
                (2, 'Own SEK B', 'own', 1),
                (3, 'Own USD', 'own', 2),
                (4, 'External', 'external', NULL);
            ",
        )
        .unwrap();
        conn
    }

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
    }

    #[test]
    fn same_currency_own_to_own_rejects_opposing_amount() {
        let conn = test_db();
        assert!(validate_transaction(&conn, 1, 2, None).is_ok());
        assert!(validate_transaction(&conn, 1, 2, Some(500)).is_err());
    }

    #[test]
    fn cross_currency_own_to_own_requires_opposing_amount() {
        let conn = test_db();
        assert!(validate_transaction(&conn, 1, 3, Some(500)).is_ok());
        assert!(validate_transaction(&conn, 1, 3, None).is_err());
    }

    #[test]
    fn own_to_external_rejects_opposing_amount() {
        let conn = test_db();
        assert!(validate_transaction(&conn, 1, 4, None).is_ok());
        assert!(validate_transaction(&conn, 1, 4, Some(500)).is_err());
    }

    #[test]
    fn external_as_account_id_is_rejected() {
        let conn = test_db();
        assert!(validate_transaction(&conn, 4, 1, None).is_err());
    }

    #[test]
    fn nonexistent_account_is_rejected() {
        let conn = test_db();
        assert!(validate_transaction(&conn, 99, 1, None).is_err());
        assert!(validate_transaction(&conn, 1, 99, None).is_err());
    }

    #[test]
    fn unmatched_route_is_404_with_error_envelope() {
        let conn = setup();
        let response = route(&conn, &Method::Get, "/nope", "");
        assert_eq!(response.status_code().0, 404);
    }

    #[test]
    fn duplicate_primary_key_maps_to_409() {
        let conn = setup();
        conn.execute("INSERT INTO categories (id, name) VALUES (1, 'Food')", [])
            .unwrap();
        let err = conn
            .execute("INSERT INTO categories (id, name) VALUES (1, 'Drinks')", [])
            .unwrap_err();
        assert_eq!(map_db_error(&err), (409, "duplicate id".to_string()));
    }

    #[test]
    fn check_violation_maps_to_400() {
        let conn = setup();
        let err = conn
            .execute(
                "INSERT INTO accounts (id, name, type, currency_id) VALUES (1, 'Wallet', 'own', NULL)",
                [],
            )
            .unwrap_err();
        let (status, _) = map_db_error(&err);
        assert_eq!(status, 400);
    }

    #[test]
    fn create_account_returns_201_with_row() {
        let conn = test_db();
        let response = route(&conn, &Method::Post, "/accounts", r#"{"name": "New", "type": "own", "currency_id": 1}"#);
        assert_eq!(response.status_code().0, 201);
    }

    #[test]
    fn create_own_account_without_currency_is_400() {
        let conn = test_db();
        let response = route(&conn, &Method::Post, "/accounts", r#"{"name": "New", "type": "own"}"#);
        assert_eq!(response.status_code().0, 400);
    }

    #[test]
    fn create_external_account_with_currency_is_400() {
        let conn = test_db();
        let response = route(
            &conn,
            &Method::Post,
            "/accounts",
            r#"{"name": "New", "type": "external", "currency_id": 1}"#,
        );
        assert_eq!(response.status_code().0, 400);
    }

    #[test]
    fn list_accounts_returns_200() {
        let conn = test_db();
        let response = route(&conn, &Method::Get, "/accounts", "");
        assert_eq!(response.status_code().0, 200);
    }

    #[test]
    fn delete_account_returns_204() {
        let conn = test_db();
        let response = route(&conn, &Method::Delete, "/accounts/4", "");
        assert_eq!(response.status_code().0, 204);
    }

    #[test]
    fn delete_unknown_account_returns_404() {
        let conn = test_db();
        let response = route(&conn, &Method::Delete, "/accounts/999", "");
        assert_eq!(response.status_code().0, 404);
    }

    #[test]
    fn delete_referenced_account_returns_400() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id) VALUES ('t1', 'x', '2024-01-01', 100, 1, 4)",
            [],
        )
        .unwrap();
        let response = route(&conn, &Method::Delete, "/accounts/1", "");
        assert_eq!(response.status_code().0, 400);
    }

    #[test]
    fn create_and_list_currencies() {
        let conn = setup();
        let response = route(
            &conn,
            &Method::Post,
            "/currencies",
            r#"{"code": "SEK", "minor_unit": 2}"#,
        );
        assert_eq!(response.status_code().0, 201);

        let response = route(&conn, &Method::Get, "/currencies", "");
        assert_eq!(response.status_code().0, 200);
    }

    #[test]
    fn duplicate_currency_code_is_409() {
        let conn = setup();
        let body = r#"{"code": "SEK", "minor_unit": 2}"#;
        assert_eq!(route(&conn, &Method::Post, "/currencies", body).status_code().0, 201);
        assert_eq!(route(&conn, &Method::Post, "/currencies", body).status_code().0, 409);
    }

    #[test]
    fn create_list_and_delete_category() {
        let conn = setup();
        let response = route(&conn, &Method::Post, "/categories", r#"{"name": "Food"}"#);
        assert_eq!(response.status_code().0, 201);

        let response = route(&conn, &Method::Get, "/categories", "");
        assert_eq!(response.status_code().0, 200);

        let response = route(&conn, &Method::Delete, "/categories/1", "");
        assert_eq!(response.status_code().0, 204);

        let response = route(&conn, &Method::Delete, "/categories/1", "");
        assert_eq!(response.status_code().0, 404);
    }

    #[test]
    fn deleting_category_nulls_referencing_transactions() {
        let conn = setup();
        conn.execute_batch(
            "
            INSERT INTO currencies (id, code, minor_unit) VALUES (1, 'SEK', 2);
            INSERT INTO accounts (id, name, type, currency_id) VALUES
                (1, 'Own', 'own', 1), (2, 'External', 'external', NULL);
            INSERT INTO categories (id, name) VALUES (1, 'Food');
            INSERT INTO transactions
                (id, description, date, amount, account_id, opposing_account_id, category_id)
                VALUES ('t1', 'lunch', '2024-01-01', -100, 1, 2, 1);
            ",
        )
        .unwrap();

        let response = route(&conn, &Method::Delete, "/categories/1", "");
        assert_eq!(response.status_code().0, 204);

        let category_id: Option<i64> = conn
            .query_row("SELECT category_id FROM transactions WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(category_id, None);
    }

    #[test]
    fn db_error_response_uses_error_envelope() {
        let conn = setup();
        conn.execute("INSERT INTO categories (id, name) VALUES (1, 'Food')", [])
            .unwrap();
        let err = conn
            .execute("INSERT INTO categories (id, name) VALUES (1, 'Drinks')", [])
            .unwrap_err();
        let response = db_error_response(&err);
        assert_eq!(response.status_code().0, 409);
    }

    fn response_body(response: Response<Cursor<Vec<u8>>>) -> String {
        String::from_utf8(response.into_reader().into_inner()).unwrap()
    }

    const TXN_BODY: &str = r#"{"id": "t1", "description": "Coffee", "date": "2024-01-01", "amount": -500, "account_id": 1, "opposing_account_id": 4}"#;

    #[test]
    fn create_transaction_returns_201_with_row() {
        let conn = test_db();
        let response = route(&conn, &Method::Post, "/transactions", TXN_BODY);
        assert_eq!(response.status_code().0, 201);
        assert!(response_body(response).contains(r#""id": "t1""#));
    }

    #[test]
    fn create_transaction_validation_failure_is_400() {
        let conn = test_db();
        // account_id must be 'own'; account 4 is 'external'.
        let body = r#"{"id": "t1", "description": "Coffee", "date": "2024-01-01", "amount": -500, "account_id": 4, "opposing_account_id": 1}"#;
        let response = route(&conn, &Method::Post, "/transactions", body);
        assert_eq!(response.status_code().0, 400);

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn duplicate_transaction_id_is_409() {
        let conn = test_db();
        assert_eq!(route(&conn, &Method::Post, "/transactions", TXN_BODY).status_code().0, 201);
        assert_eq!(route(&conn, &Method::Post, "/transactions", TXN_BODY).status_code().0, 409);
    }

    #[test]
    fn batch_insert_succeeds_and_shares_batch_id() {
        let conn = test_db();
        let body = r#"{
            "batch_id": "b1",
            "transactions": [
                {"id": "t1", "description": "Coffee", "date": "2024-01-01", "amount": -500, "account_id": 1, "opposing_account_id": 4},
                {"id": "t2", "description": "Lunch", "date": "2024-01-02", "amount": -1000, "account_id": 1, "opposing_account_id": 4}
            ]
        }"#;
        let response = route(&conn, &Method::Post, "/transactions/batch", body);
        assert_eq!(response.status_code().0, 201);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM transactions WHERE batch_id = 'b1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn batch_rolls_back_entirely_on_one_invalid_row() {
        let conn = test_db();
        let body = r#"{
            "batch_id": "b1",
            "transactions": [
                {"id": "t1", "description": "Coffee", "date": "2024-01-01", "amount": -500, "account_id": 1, "opposing_account_id": 4},
                {"id": "t2", "description": "Bad", "date": "2024-01-02", "amount": -1000, "account_id": 4, "opposing_account_id": 1}
            ]
        }"#;
        let response = route(&conn, &Method::Post, "/transactions/batch", body);
        assert_eq!(response.status_code().0, 400);

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0, "no row from a rolled-back batch should persist");
    }

    #[test]
    fn batch_rolls_back_entirely_on_duplicate_id() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id) VALUES ('t1', 'x', '2024-01-01', -100, 1, 4)",
            [],
        )
        .unwrap();
        let body = r#"{
            "batch_id": "b1",
            "transactions": [
                {"id": "t2", "description": "Coffee", "date": "2024-01-02", "amount": -500, "account_id": 1, "opposing_account_id": 4},
                {"id": "t1", "description": "Dup", "date": "2024-01-03", "amount": -1000, "account_id": 1, "opposing_account_id": 4}
            ]
        }"#;
        let response = route(&conn, &Method::Post, "/transactions/batch", body);
        assert_eq!(response.status_code().0, 409);

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "only the pre-existing row should remain; t2 must not persist");
    }

    #[test]
    fn list_transactions_orders_by_date_desc_then_id() {
        let conn = test_db();
        conn.execute_batch(
            "
            INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id) VALUES
                ('a', 'x', '2024-01-01', -100, 1, 4),
                ('b', 'x', '2024-01-03', -100, 1, 4),
                ('c', 'x', '2024-01-02', -100, 1, 4);
            ",
        )
        .unwrap();

        let response = route(&conn, &Method::Get, "/transactions", "");
        assert_eq!(response.status_code().0, 200);
        let body = response_body(response);
        let pos_a = body.find(r#""id": "a""#).unwrap();
        let pos_b = body.find(r#""id": "b""#).unwrap();
        let pos_c = body.find(r#""id": "c""#).unwrap();
        assert!(pos_b < pos_c && pos_c < pos_a, "expected order b, c, a but got: {body}");
    }

    #[test]
    fn patch_updates_description_and_category_id() {
        let conn = test_db();
        conn.execute_batch(
            "
            INSERT INTO categories (id, name) VALUES (1, 'Food');
            INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id)
                VALUES ('t1', 'Old', '2024-01-01', -100, 1, 4);
            ",
        )
        .unwrap();

        let response = route(
            &conn,
            &Method::Patch,
            "/transactions/t1",
            r#"{"description": "New", "category_id": 1}"#,
        );
        assert_eq!(response.status_code().0, 200);

        let (description, category_id): (String, Option<i64>) = conn
            .query_row("SELECT description, category_id FROM transactions WHERE id = 't1'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(description, "New");
        assert_eq!(category_id, Some(1));
    }

    #[test]
    fn patch_rejecting_immutable_field_is_400() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id) VALUES ('t1', 'Old', '2024-01-01', -100, 1, 4)",
            [],
        )
        .unwrap();

        let response = route(&conn, &Method::Patch, "/transactions/t1", r#"{"amount": 500}"#);
        assert_eq!(response.status_code().0, 400);

        let description: String = conn
            .query_row("SELECT description FROM transactions WHERE id = 't1'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(description, "Old", "rejected PATCH must not have mutated the row");
    }

    #[test]
    fn patch_unknown_id_is_404() {
        let conn = test_db();
        let response = route(&conn, &Method::Patch, "/transactions/nope", r#"{"description": "New"}"#);
        assert_eq!(response.status_code().0, 404);
    }

    #[test]
    fn delete_transaction_returns_204() {
        let conn = test_db();
        conn.execute(
            "INSERT INTO transactions (id, description, date, amount, account_id, opposing_account_id) VALUES ('t1', 'x', '2024-01-01', -100, 1, 4)",
            [],
        )
        .unwrap();

        let response = route(&conn, &Method::Delete, "/transactions/t1", "");
        assert_eq!(response.status_code().0, 204);

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn delete_unknown_transaction_is_404() {
        let conn = test_db();
        let response = route(&conn, &Method::Delete, "/transactions/nope", "");
        assert_eq!(response.status_code().0, 404);
    }
}
