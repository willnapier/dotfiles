# Build stage
FROM rust:1.86-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY . .

# Build only the portal binary from the workspace.
# `-p clinical-portal` keeps notes-only deps (regex, glob, etc.) out of
# the build entirely, so the Docker image stays minimal.
RUN cargo build --release -p clinical-portal --bin clinical-portal

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/clinical-portal /usr/local/bin/clinical-portal

# Storage and database on persistent volume
RUN mkdir -p /data/storage

WORKDIR /data
EXPOSE 3849

CMD ["clinical-portal", "serve", "--port", "3849", "--db", "/data/clinical-portal.db"]
