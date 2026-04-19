FROM rust:1.87-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p transcriptd

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/transcriptd /usr/local/bin/
VOLUME /data
ENV TRANSCRIPTD_DB=/data/transcriptd.db
ENTRYPOINT ["transcriptd"]
