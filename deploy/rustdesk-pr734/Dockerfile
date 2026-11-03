ARG RUST_IMAGE=public.ecr.aws/docker/library/rust:1.93.1-bookworm@sha256:7c4ae649a84014c467d79319bbf17ce2632ae8b8be123ac2fb2ea5be46823f31
ARG RUNTIME_IMAGE=public.ecr.aws/docker/library/debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
FROM ${RUST_IMAGE} AS build
RUN apt-get update && apt-get install --no-install-recommends -y pkg-config libsodium-dev libssl-dev libx11-dev \
    && rm -rf /var/lib/apt/lists/*
ENV CARGO_HOME=/cargo SODIUM_USE_PKG_CONFIG=1
COPY --from=cargo_registry / /cargo/registry/
WORKDIR /build
COPY Cargo.toml Cargo.lock build.rs ./
COPY .env db_v2.sqlite3 ./
COPY src/ ./src/
COPY tests/ ./tests/
COPY libs/hbb_common/ ./libs/hbb_common/
COPY vendor-git/ ./vendor-git/
COPY .cargo/config.offline.toml ./.cargo/config.toml
RUN --network=none --mount=type=cache,id=rustdesk-pr734-ef2166f7-amd64-target,target=/build/target,sharing=locked \
    cargo test --release --locked --offline --test key_exchange --test compat -j 4
RUN --network=none --mount=type=cache,id=rustdesk-pr734-ef2166f7-amd64-target,target=/build/target,sharing=locked \
    cargo build --release --locked --offline --bin hbbs -j 4 \
    && mkdir -p /out && cp target/release/hbbs /out/hbbs

FROM ${RUNTIME_IMAGE}
LABEL org.opencontainers.image.version="pr734-ef2166f7" \
    org.opencontainers.image.description="RustDesk server PR 734 fixed commit; independent build" \
    org.opencontainers.image.revision="ef2166f7760905a18b6588f0882d633d3688ad2f"
RUN apt-get update && apt-get install --no-install-recommends -y ca-certificates libsodium23 libssl3 libx11-6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /out/hbbs /usr/local/bin/hbbs
COPY LICENSE /usr/share/doc/rustdesk-pr734/LICENSE
COPY deploy-entrypoint.sh /usr/local/bin/hbbs-entrypoint
RUN chmod 0755 /usr/local/bin/hbbs-entrypoint
WORKDIR /root
EXPOSE 21115/tcp 21116/tcp 21116/udp 21118/tcp
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/hbbs-entrypoint"]
