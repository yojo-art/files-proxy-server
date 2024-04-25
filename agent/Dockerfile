FROM rust:alpine
RUN apk add --no-cache musl-dev curl
RUN curl -sSL https://github.com/mozilla/sccache/releases/download/0.2.13/sccache-0.2.13-x86_64-unknown-linux-musl.tar.gz | tar -zxf - -C /tmp && mv /tmp/sccache*/sccache /usr/local/bin && rm -rf /tmp/sccache*
ENV CARGO_HOME=/var/cache/cargo
RUN mkdir /app
WORKDIR /app
COPY src ./src
COPY Cargo.toml ./Cargo.toml
ENV RUSTC_WRAPPER=/usr/local/bin/sccache
ENV SCCACHE_DIR=/var/cache/sccache
RUN --mount=type=cache,target=/var/cache/cargo --mount=type=cache,target=/var/cache/sccache cargo build --target x86_64-unknown-linux-musl --release

FROM alpine:latest
ARG UID="850"
ARG GID="850"
RUN addgroup -g "${GID}" agent && adduser -u "${UID}" -G agent -D -h /files-agent -s /bin/sh agent
WORKDIR /files-agent
USER agent
COPY --from=0 /app/target/x86_64-unknown-linux-musl/release/agent ./files-agent
CMD ["./files-agent"]
