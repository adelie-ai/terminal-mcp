# syntax=docker/dockerfile:1

# Run tests inside a Rust container
FROM rust:1.92-trixie
WORKDIR /repo

# Cache deps first
COPY Cargo.toml Cargo.lock ./

COPY src/lib.rs src/main.rs ./src/

RUN cargo fetch --locked

COPY src ./src
COPY tests ./tests

ENV RUST_BACKTRACE=1

CMD ["cargo", "test", "--locked"]
