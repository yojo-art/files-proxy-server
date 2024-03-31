FROM rust:alpine
RUN apk add --no-cache musl-dev
RUN --mount=type=cache,target=/cargo-home
ENV CARGO_HOME=/cargo-home
RUN mkdir /app
WORKDIR /app
COPY src ./src
COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock
RUN cargo build --target x86_64-unknown-linux-musl --release

FROM alpine:latest
WORKDIR /root/
COPY --from=0 /app/target/x86_64-unknown-linux-musl/release/server ./
EXPOSE 8080
CMD ["./server"]
