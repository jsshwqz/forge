# Aion Forge server —— 多阶段构建（构建器与运行时分离，镜像最小化）
# 构建: docker/podman build -f deploy/Dockerfile -t aionforge/server:latest .
# 说明: 无 Dockerfile 之外的新增 cargo 依赖；运行时镜像仅含二进制与 CA 证书。

FROM rust:1.94-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

# 先拷贝清单以利用层缓存
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY core ./core
COPY execution ./execution
COPY planning ./planning
COPY verification ./verification
COPY recovery ./recovery
COPY capability ./capability
COPY products ./products
COPY knowledge ./knowledge
COPY tools ./tools
COPY server ./server
COPY storage ./storage
COPY sdk ./sdk
COPY cli ./cli
COPY tests ./tests

RUN cargo build --release --bin forge-server

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/forge-server /usr/local/bin/forge-server

# 非 root 运行
RUN useradd -r -u 10001 forge
USER forge

ENV FORGE_PORT=8080 \
    FORGE_HOST=0.0.0.0 \
    RUST_LOG=info
# 必须由部署方注入: FORGE_API_KEY / FORGE_PG_URL / FORGE_CORS_ORIGINS(可选) / S3 与 LLM 相关(可选)
EXPOSE 8080

# 启动即执行 sqlx 迁移(connect_and_migrate)，升级路径 = 换镜像重启
ENTRYPOINT ["/usr/local/bin/forge-server"]
