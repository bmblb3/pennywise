use rusqlite::Connection;
use tiny_http::{Response, Server};

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

fn main() {
    let config = Config::from_args();

    let conn = open_db(&config.db);
    migrate(&conn);

    let server = Server::http(&config.bind).expect("failed to bind server");
    println!("listening on {}", config.bind);

    for request in server.incoming_requests() {
        let response = Response::from_string("Not Implemented").with_status_code(501);
        let _ = request.respond(response);
    }
}
