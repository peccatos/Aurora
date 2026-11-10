# Aurora API (OAuth BFF) — container for PaaS deploys (Render, Fly, etc.).
#
# Build context: repository root. The image contains the aurora-api server
# binary AND the built web shell: serving both from one service keeps the
# browser on a single origin, so the session/CSRF cookies are first-party
# (third-party cookie blocking makes a split frontend/API deployment
# unworkable in modern browsers). The desktop shell (Tauri) is not part of
# this image.
#
# yt-dlp note: this image intentionally does NOT bundle yt-dlp. The media
# endpoints degrade gracefully: GET /api/media/ping reports
# {"available":false} (503) and POST /api/media/metadata answers 503, and
# the UI hides the feature. Bundling it is a deliberate per-deployment
# decision (license/size/upgrade cadence), not a default.

FROM node:22-slim AS web
WORKDIR /web
# Build the browser bundle. The API base URL is same-origin by default, so
# no build-time env is needed; VITE_AURORA_* overrides remain available.
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
# The web shell's legal page imports docs/legal/*.md via Vite ?raw with a
# path relative to the REPO ROOT (frontend/src/pages -> ../../../docs).
# In this container the frontend tree lives at /web, so the import target
# is /docs — docs must be copied one level ABOVE the workdir, mirroring
# the repo layout.
COPY docs /docs
COPY frontend ./
# The generated wasm glue is a local artifact; keep it out of the context.
RUN npm run build

FROM rust:1.94 AS builder
WORKDIR /app

# The workspace manifest lists all members, so the whole crates/ tree has to
# be present even though only the aurora-api closure is compiled.
COPY Cargo.toml Cargo.lock ./
# vendor/sqlx-sqlite is a [patch.crates-io] path dependency (CVE override);
# it must be present in the image or cargo cannot resolve the patch.
COPY vendor ./vendor
COPY crates ./crates
# The workspace manifest lists apps/desktop as a member, but the Tauri shell
# is out of scope here. An empty stub manifest (just [package]) satisfies the
# workspace loader; cargo only compiles the aurora-api closure anyway.
RUN mkdir -p apps/desktop/src && printf '[package]\nname = "aurora-desktop"\nversion = "0.2.0"\nedition = "2021"\n' > apps/desktop/Cargo.toml \
    && touch apps/desktop/src/main.rs

RUN cargo build --release -p aurora-api --bin aurora-api

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
# SQLite writes to a volume; run as an unprivileged user.
RUN useradd --system --home-dir /data --create-home aurora \
    && mkdir -p /data && chown aurora:aurora /data
WORKDIR /app
COPY --from=builder /app/target/release/aurora-api /usr/local/bin/aurora-api
# Web shell, served by the API at the SPA fallback (see lib.rs ServeDir).
COPY --from=web /web/dist /usr/share/aurora/web
ENV AURORA_DATABASE_PATH=/data/aurora.db
USER aurora
EXPOSE 8080
# Render sets PORT; main.rs binds 0.0.0.0:$PORT (default 8080).
CMD ["aurora-api"]
