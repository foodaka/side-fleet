# syntax=docker/dockerfile:1.7
# The Sidefleet control plane as a container (the hosted service, or any
# self-hosted deployment that prefers containers). Workers are not run here.
#
#   docker build --build-arg SIDEFLEET_GIT_SHA=$(git rev-parse --short=9 HEAD) -t sidefleet .
#   docker run -p 8080:8080 -e DATABASE_URL=... -e SIDEFLEET_GITHUB_CLIENT_ID=... sidefleet
#
# The web app is prebuilt in crates/server/web-dist (committed), so no Node here.

FROM rust:1-bookworm AS build
WORKDIR /src
ARG SIDEFLEET_GIT_SHA=""
ENV SIDEFLEET_GIT_SHA=${SIDEFLEET_GIT_SHA}
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p sidefleet \
    && cp target/release/sidefleet /usr/local/bin/sidefleet \
    && strip /usr/local/bin/sidefleet

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --create-home --uid 10001 sidefleet
COPY --from=build /usr/local/bin/sidefleet /usr/local/bin/sidefleet
USER sidefleet
WORKDIR /home/sidefleet
ENV SIDEFLEET_BIND=0.0.0.0:8080 \
    RUST_LOG=sidefleet=info,sidefleet_server=info,tower_http=warn \
    SIDEFLEET_LOG_FORMAT=json
EXPOSE 8080
HEALTHCHECK --interval=15s --timeout=3s --start-period=20s CMD curl -fsS http://127.0.0.1:8080/healthz || exit 1
# --no-worker: the hosted control plane never runs agents itself.
CMD ["sidefleet", "serve", "--no-worker"]
