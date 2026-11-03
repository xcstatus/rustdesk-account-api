# Build from the patched, pinned 1.1.16 checkout, with cargo vendor output.
ARG RUST_IMAGE=public.ecr.aws/docker/library/rust:1.93.1-bookworm@sha256:7c4ae649a84014c467d79319bbf17ce2632ae8b8be123ac2fb2ea5be46823f31
ARG RUNTIME_IMAGE=public.ecr.aws/docker/library/debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
FROM ${RUST_IMAGE} AS build
RUN apt-get update && apt-get install --no-install-recommends -y pkg-config libsodium-dev \
    && rm -rf /var/lib/apt/lists/*
ENV SODIUM_USE_PKG_CONFIG=1
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY src/ ./src/
COPY tests/ ./tests/
COPY examples/ ./examples/
COPY libs/hbb_common/ ./libs/hbb_common/
COPY vendor-all/ ./vendor-all/
COPY .cargo/config.offline.toml ./.cargo/config.toml
RUN --network=none cargo test --release --locked --offline --lib account_secure_tcp -j 2
RUN --network=none cargo test --release --locked --offline --test account_secure_tcp -j 2 -- --test-threads=1
RUN --network=none cargo build --release --locked --offline --bin hbbs -j 2
RUN --network=none cargo build --release --locked --offline --example account_secure_tcp_probe -j 2

FROM ${RUNTIME_IMAGE}
LABEL org.opencontainers.image.version="1.1.16-account-compat-v1" \
    org.opencontainers.image.description="Independent opt-in hbbs account handshake compatibility patch" \
    io.rustdesk-account.upstream-revision="73523b31cfd25d77dee862e6fc9f5e1fb5e485ef"
RUN apt-get update && apt-get install --no-install-recommends -y ca-certificates libsodium23 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /build/target/release/hbbs /usr/local/bin/hbbs
COPY --from=build /build/target/release/examples/account_secure_tcp_probe /usr/local/bin/hbbs-account-probe
COPY LICENSE /usr/share/doc/hbbs-account-compat/LICENSE
WORKDIR /root
EXPOSE 21115/tcp 21116/tcp 21116/udp 21118/tcp
STOPSIGNAL SIGTERM
ENTRYPOINT ["hbbs"]
