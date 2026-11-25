FROM node:24-bookworm-slim AS node-dependencies
WORKDIR /source
COPY package.json package-lock.json ./
RUN --mount=type=cache,id=codex-mobile-npm,target=/root/.npm npm ci --no-audit --no-fund

FROM node-dependencies AS mobile
COPY apps/mobile/ apps/mobile/
COPY shared/ shared/
RUN CODEX_MOBILE_RELAY_WEB=1 npm run build:mobile

FROM node-dependencies AS frontend
COPY apps/mobile/ apps/mobile/
COPY apps/desktop/ apps/desktop/
COPY shared/ shared/
RUN npm run build:web

FROM scratch AS frontend-export
COPY --from=frontend /source/dist/ /dist/

# 与 Cargo.toml 声明的 Rust 最低版本保持一致。
FROM rust:1.90-bookworm AS rust-toolchain
WORKDIR /source

FROM rust-toolchain AS build-relay
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY apps/desktop/src-tauri/ apps/desktop/src-tauri/
RUN cargo build --locked --release -p codex-mobile-relay

FROM debian:bookworm-slim AS relay
COPY --from=build-relay /source/target/release/codex-mobile-relay /usr/local/bin/
RUN install -d -m 0700 -o 65534 -g 65534 /data
USER 65534:65534
ENTRYPOINT ["codex-mobile-relay"]

FROM rust-toolchain AS check-source
COPY --from=node-dependencies /usr/local/ /usr/local/
RUN rustup component add rustfmt clippy
COPY --from=node-dependencies /source/node_modules/ /source/node_modules/
COPY . .
COPY --from=frontend /source/dist/ /source/dist/
# 通用检查只编译桥接与中转；原生桌面仍在 Windows/macOS CI 验证。
# 缓存交给 BuildKit 回收，不写入宿主项目，也不保留增量和调试符号。
ENV CARGO_TARGET_DIR=/build/target \
    CARGO_INCREMENTAL=0 \
    CARGO_PROFILE_DEV_DEBUG=0 \
    CARGO_PROFILE_TEST_DEBUG=0

FROM check-source AS check
# 只让测试命令重新执行，保留依赖和编译缓存挂载。
ARG CHECK_RUN
RUN --mount=type=cache,id=codex-mobile-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=codex-mobile-cargo-git,target=/usr/local/cargo/git \
    --mount=type=cache,id=codex-mobile-cargo-target,target=/build/target,sharing=locked \
    npm run release:check \
    && cargo fmt --all -- --check \
    && npm run test:native \
    && npm run test:relay \
    && cargo check --locked -p codex-mobile-bridge -p codex-mobile-relay --all-targets \
    && cargo clippy --locked -p codex-mobile-bridge -p codex-mobile-relay --all-targets -- -D warnings

FROM caddy:2-alpine AS web
COPY --from=mobile /source/dist/mobile/ /srv/codex-mobile/
COPY deploy/relay/Caddyfile /etc/caddy/Caddyfile
