# Aurora API (OAuth BFF) — container for PaaS deploys (Render, Fly, etc.).
#
# Build context: repository root. Only the aurora-api server binary is
# produced; the desktop shell (Tauri) and the frontend static site are not
# part of this image.
#
# yt-dlp note: this image intentionally does NOT bundle yt-dlp. The media
# endpoints degrade gracefully: GET /api/media/ping reports
# {"available":false} (503) and POST /api/media/metadata answers 503, and
# the UI hides the feature. Bundling it is a deliberate per-deployment
# decision (license/size/upgrade cadence), not a default.

FROM rust:1.90 AS builder
WORKDIR /app

# The workspace manifest lists all members, so the whole crates/ tree has to
# be present even though only the aurora-api closure is compiled.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# The workspace manifest lists apps/desktop as a member, but the Tauri shell
# is out of scope here. An empty stub manifest (just [package]) satisfies the
# workspace loader; cargo only compiles the aurora-api closure anyway.
RUN mkdir -p apps/desktop/src && printf '[package]\nname = "aurora-desktop"\nversion = "0.2.0"\nedition = "2021"\n' > apps/desktop/Cargo.toml \
    && touch apps/desktop/src/main.rs

RUN cargo build --release -p aurora-api --bin aurora-api

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
# SQLite writes to a volume; run as an unprivileged user.
RUN useradd --system --home /data aurora
WORKDIR /app
COPY --from=builder /app/target/release/aurora-api /usr/local/bin/aurora-api
ENV AURORA_DATABASE_PATH=/data/aurora.db
USER aurora
EXPOSE 8080
# Render sets PORT; main.rs binds 0.0.0.0:$PORT (default 8080).
CMD ["aurora-api"]
