#!/bin/sh
# Runs the package's tests with the live-server ones on: starts a throwaway Postgres and a real
# `sidefleet serve` on spare ports, then removes them. Needs Docker and a `sidefleet` binary.
# On a Mac: swift test. On Linux, SWIFT_DOCKER=1 runs swift from the official image.
set -eu
here="$(cd "$(dirname "$0")/.." && pwd)"
bin="${SIDEFLEET_BIN:-sidefleet}"
pg_port="${SF_TEST_PG_PORT:-55433}"
port="${SF_TEST_PORT:-57373}"
name="sf-swift-test-pg-$$"
conf="$(mktemp -d)"
key="test-dev-key-$$"
token="test-worker-token-$$"

docker run -d --rm --name "$name" -e POSTGRES_USER=sidefleet -e POSTGRES_PASSWORD=sidefleet -e POSTGRES_DB=sidefleet \
  -p "127.0.0.1:$pg_port:5432" postgres:16-alpine >/dev/null
server=""
cleanup() {
  [ -n "$server" ] && kill "$server" 2>/dev/null || true
  docker rm -f "$name" >/dev/null 2>&1 || true
  rm -rf "$conf"
}
trap cleanup EXIT
until docker exec "$name" pg_isready -U sidefleet >/dev/null 2>&1; do sleep 0.3; done
sleep 1

SIDEFLEET_CONFIG_DIR="$conf" "$bin" serve --no-worker --bind "127.0.0.1:$port" \
  --database-url "postgres://sidefleet:sidefleet@127.0.0.1:$pg_port/sidefleet" \
  --dev-key "$key" --worker-tokens "$token" >"$conf/serve.log" 2>&1 &
server=$!
i=0
until curl -fsS "http://127.0.0.1:$port/healthz" >/dev/null 2>&1; do
  i=$((i + 1))
  if [ "$i" -gt 60 ]; then cat "$conf/serve.log"; exit 1; fi
  sleep 0.5
done

export SIDEFLEET_TEST_URL="http://127.0.0.1:$port" SIDEFLEET_TEST_DEV_KEY="$key" SIDEFLEET_TEST_WORKER_TOKEN="$token"
cd "$here"
if [ "${SWIFT_DOCKER:-}" = 1 ]; then
  build="${SWIFT_BUILD_DIR:-/tmp/sf-swift-build}"
  docker run --rm --network host -u "$(id -u):$(id -g)" -e HOME="$build/home" \
    -e SIDEFLEET_TEST_URL -e SIDEFLEET_TEST_DEV_KEY -e SIDEFLEET_TEST_WORKER_TOKEN \
    -v "$here":/src -v "$build":"$build" -w /src swift:6.1 swift test --scratch-path "$build/out" "$@"
else
  swift test "$@"
fi
