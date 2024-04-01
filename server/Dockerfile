FROM rust:alpine
RUN apk add --no-cache musl-dev curl
RUN curl -sSL https://github.com/mozilla/sccache/releases/download/0.2.13/sccache-0.2.13-x86_64-unknown-linux-musl.tar.gz | tar -zxf - -C /tmp && mv /tmp/sccache*/sccache /usr/local/bin && rm -rf /tmp/sccache*
ENV CARGO_HOME=/var/cache/cargo
RUN mkdir /app
WORKDIR /app
COPY src ./src
COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock
RUN --mount=type=cache,target=/var/cache/cargo cargo fetch --locked
ENV RUSTC_WRAPPER=/usr/local/bin/sccache
ENV SCCACHE_DIR=/var/cache/sccache
RUN --mount=type=cache,target=/var/cache/cargo --mount=type=cache,target=/var/cache/sccache cargo build --target x86_64-unknown-linux-musl --release --offline

FROM alpine:latest
ARG UID="851"
ARG GID="851"
RUN addgroup -g "${GID}" proxy && adduser -u "${UID}" -G proxy -D -h /files-proxy -s /bin/sh proxy
WORKDIR /files-proxy
USER proxy
COPY --from=0 /app/target/x86_64-unknown-linux-musl/release/server ./files-proxy
EXPOSE 8080
EXPOSE 23725
CMD ["./files-proxy"]
