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
#[allow(dead_code)] // consumed by the transactions endpoints (#7), not yet wired up
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

// --- Routing ----------------------------------------------------------------
//
// Dispatch skeleton: matches method + path against the known resource
// routes. No handlers exist yet (they land in #5/#6/#7), so every arm is a
// 404 stub for now; each ticket swaps its arm's body for a real call.

fn route(method: &Method, path: &str) -> Response<Cursor<Vec<u8>>> {
    let segments: Vec<&str> = path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match (method, segments.as_slice()) {
        (Method::Post, ["currencies"]) => not_found(),
        (Method::Get, ["currencies"]) => not_found(),

        (Method::Post, ["accounts"]) => not_found(),
        (Method::Get, ["accounts"]) => not_found(),
        (Method::Delete, ["accounts", _id]) => not_found(),

        (Method::Post, ["categories"]) => not_found(),
        (Method::Get, ["categories"]) => not_found(),
        (Method::Delete, ["categories", _id]) => not_found(),

        (Method::Post, ["transactions", "batch"]) => not_found(),
        (Method::Post, ["transactions"]) => not_found(),
        (Method::Get, ["transactions"]) => not_found(),
        (Method::Patch, ["transactions", _id]) => not_found(),
        (Method::Delete, ["transactions", _id]) => not_found(),

        _ => not_found(),
    }
}

fn main() {
    let config = Config::from_args();

    let conn = open_db(&config.db);
    migrate(&conn);
    let _ = conn; // wired up by resource-endpoint tickets

    let server = Server::http(&config.bind).expect("failed to bind server");
    println!("listening on {}", config.bind);

    for request in server.incoming_requests() {
        let method = request.method().clone();
        let path = request.url().split('?').next().unwrap_or("").to_string();
        let response = route(&method, &path);
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
        let response = route(&Method::Get, "/nope");
        assert_eq!(response.status_code().0, 404);
    }

    #[test]
    fn known_route_is_stubbed_404_for_now() {
        let response = route(&Method::Get, "/currencies");
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
}
