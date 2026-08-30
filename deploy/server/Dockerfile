FROM rust:1.92.0-bookworm AS builder

ENV RUSTUP_TOOLCHAIN=1.92.0
WORKDIR /build
COPY . .
RUN cargo build --locked --release --package strajer-server

FROM debian:bookworm-slim AS runtime

RUN groupadd --gid 10001 strajer \
    && useradd --uid 10001 --gid strajer --no-create-home --shell /usr/sbin/nologin strajer

COPY --from=builder /build/target/release/strajer-server /usr/local/bin/strajer-server

USER 10001:10001
EXPOSE 8080

ENV STRAJER_BIND_ADDR=0.0.0.0:8080
ENV STRAJER_HEALTHCHECK_ADDR=127.0.0.1:8080
ENV RUST_LOG=strajer_server=info,tower_http=info

HEALTHCHECK --interval=10s --timeout=3s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/strajer-server", "healthcheck"]

ENTRYPOINT ["/usr/local/bin/strajer-server"]
