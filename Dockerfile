FROM rust:1.99.1-trixie AS build
WORKDIR /app

# Cache dependencies separately from source code
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs
RUN cargo build --release --locked && rm -rf src

# Build the actual application
COPY src ./src
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian13
COPY --from=build /app/target/release/rust-operator /usr/local/bin/rust-operator
ENTRYPOINT ["/usr/local/bin/rust-operator"]
