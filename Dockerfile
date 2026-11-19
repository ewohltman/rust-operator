FROM rust:1.99.1-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM gcr.io/distroless/cc-debian13
COPY --from=build /src/target/release/rust-operator /rust-operator
ENTRYPOINT ["/rust-operator"]
