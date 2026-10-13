#!/usr/bin/env bash
# Builds, checks, tests and smoke-runs Hostly on Debian Linux inside Docker, from
# any machine with Docker: `npm run check:linux`. Results (test log, app log and
# a screenshot of the running app) land in target/linux-check/.
set -euo pipefail

if [[ "${HOSTLY_IN_CONTAINER:-}" != 1 ]]; then
  root="$(cd "$(dirname "$0")/.." && pwd)"
  mkdir -p "$root/target/linux-check"
  exec docker run --rm -e HOSTLY_IN_CONTAINER=1 \
    -v "$root":/src:ro \
    -v "$root/target/linux-check":/out \
    -v hostly-linux-cargo:/usr/local/cargo/registry \
    -v hostly-linux-target:/work/target \
    rust:1-bookworm /src/scripts/linux-check.sh
fi

# Inside the container: the source is read-only at /src, results go to /out.
# The local .env is not copied in, so database tests skip here (CI runs them against PostgreSQL).
export DEBIAN_FRONTEND=noninteractive
step() { echo; echo "== $*"; }

step "system libraries"
apt-get update -qq
apt-get install -y -qq --no-install-recommends \
  libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev patchelf file \
  curl xz-utils xvfb xauth imagemagick dbus-x11 xdotool > /dev/null

step "node"
case "$(uname -m)" in aarch64) arch=arm64 ;; x86_64) arch=x64 ;; esac
curl -fsSL "https://nodejs.org/dist/v24.19.0/node-v24.19.0-linux-${arch}.tar.xz" | tar -xJ -C /opt
export PATH="/opt/node-v24.19.0-linux-${arch}/bin:$PATH"
node --version; rustc --version; uname -m

mkdir -p /work
(cd /src && tar --exclude=./node_modules --exclude=./target --exclude=./dist --exclude=./src-tauri/gen --exclude=./.env -cf - .) | tar -xf - -C /work
cd /work

step "npm ci"; npm ci --no-audit --no-fund --loglevel=error
step "typecheck"; npx tsc --noEmit && echo ok
rustup component add clippy rustfmt > /dev/null 2>&1
step "fmt"; cargo fmt --all --check && echo ok
step "clippy"; cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3
step "test"; cargo test --workspace 2>&1 | tee /out/test.log | grep -E 'test result|FAILED|panicked|^error'
step "bundle"; npx tauri build --debug --bundles deb 2>&1 | grep -E 'Finished|Bundling|error' ; ls -la target/debug/bundle/deb/

step "smoke run"
export WEBKIT_DISABLE_COMPOSITING_MODE=1 WEBKIT_DISABLE_DMABUF_RENDERER=1
dbus-run-session -- xvfb-run -a -s "-screen 0 1280x800x24" bash -c '
  ./target/debug/hostly > /out/app.log 2>&1 & app=$!
  sleep 15
  if kill -0 $app 2>/dev/null; then echo "app still running after 15s"; else wait $app; echo "app exited with $?"; fi
  import -window root /out/linux-screenshot.png && echo "screenshot saved"
  # The profile button sits at the top right of the 1200px-wide window.
  xdotool mousemove 1174 24 click 1 && sleep 2
  import -window root /out/linux-account.png && echo "account page screenshot saved"
  kill $app 2>/dev/null || true'
