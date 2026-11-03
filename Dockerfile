# Resolve/pin these tags to reviewed registry digests before a release build.
ARG NODE_IMAGE=node:22.22.1-bookworm-slim
ARG RUST_IMAGE=rust:1.93.1-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${NODE_IMAGE} AS web
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM ${RUST_IMAGE} AS api
WORKDIR /build/api
COPY api/Cargo.toml api/Cargo.lock ./
COPY api/src/ ./src/
COPY api/migrations/ ./migrations/
RUN cargo build --release --locked --bin rustdesk-account-api

FROM ${RUNTIME_IMAGE}
RUN apt-get update && apt-get install --no-install-recommends -y ca-certificates curl sqlite3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 app && useradd --uid 10001 --gid 10001 --no-create-home app \
    && mkdir -p /app/data /app/web/dist && chown -R 10001:10001 /app
WORKDIR /app
COPY --from=api --chown=10001:10001 /build/api/target/release/rustdesk-account-api /usr/local/bin/rustdesk-account-api
COPY --from=web --chown=10001:10001 /build/web/dist/ ./web/dist/
USER 10001:10001
EXPOSE 21114
STOPSIGNAL SIGTERM
ENTRYPOINT ["rustdesk-account-api"]
CMD ["serve"]
