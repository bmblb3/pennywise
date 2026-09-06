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

    fn setup() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        conn
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
