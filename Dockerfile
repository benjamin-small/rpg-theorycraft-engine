# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.88

FROM rust:${RUST_VERSION}-bookworm AS rust-builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p rtce-cli

FROM rust:${RUST_VERSION}-bookworm AS wasm-builder
WORKDIR /src
RUN rustup target add wasm32-unknown-unknown \
    && cargo install wasm-bindgen-cli --version 0.2.126 --locked
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo build --locked --release -p rtce-wasm --target wasm32-unknown-unknown \
    && wasm-bindgen \
      target/wasm32-unknown-unknown/release/rtce_wasm.wasm \
      --out-dir /wasm \
      --out-name rtce_wasm \
      --target web

FROM node:22-alpine AS web-builder
WORKDIR /src
COPY web/package.json web/package-lock.json ./web/
RUN cd web && npm ci --ignore-scripts --no-audit --no-fund
COPY web ./web
COPY crates/rtce/tests/fixtures/guide ./crates/rtce/tests/fixtures/guide
COPY --from=wasm-builder /wasm ./web/src/wasm
RUN cd web && npm run build:web && npm run standalone

FROM scratch AS standalone
COPY --from=web-builder /src/web/dist-standalone/rtce-field-guide.html /

FROM nginx:1.27-alpine AS tutorial
COPY web/nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=web-builder /src/web/dist /usr/share/nginx/html
EXPOSE 80

FROM debian:bookworm-slim AS cli
COPY --from=rust-builder /src/target/release/rtce /usr/local/bin/rtce
WORKDIR /work
USER 65532:65532
ENTRYPOINT ["rtce"]
CMD ["demo", "calc"]
