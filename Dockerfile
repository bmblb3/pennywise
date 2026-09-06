FROM rust:1.98.0-slim-bookworm AS builder

WORKDIR /build

# rusqlite's `bundled` feature compiles SQLite from C source; needs a C compiler.
RUN apt-get update && apt-get install -y --no-install-recommends gcc && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim

RUN useradd --system --create-home --uid 10001 --user-group pennywise \
    && mkdir -p /data \
    && chown pennywise:pennywise /data

COPY --from=builder /build/target/release/pennywise /usr/local/bin/pennywise

USER pennywise
WORKDIR /data
EXPOSE 8080

ENTRYPOINT ["pennywise"]
CMD ["--bind", "0.0.0.0:8080", "--db", "/data/pennywise.db"]
