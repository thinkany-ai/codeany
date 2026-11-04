#!/bin/sh
# Run the dev build and restart it whenever the source changes.
#
#   scripts/dev.sh [project-dir]
#
# Rust cannot swap code into a running GPUI app, so "hot reload" here is a
# rebuild and relaunch on save. A failed build leaves the running app alone.
set -u
cd "$(dirname "$0")/.."

PROJECT=${1:-$PWD}
# `make dev PROJECT=~/x` passes the tilde through unexpanded.
case $PROJECT in "~"*) PROJECT="$HOME${PROJECT#"~"}" ;; esac
APP_BIN=target/dscode.app/Contents/MacOS/dscode
STAMP=target/.dev-stamp
WATCH="src assets Cargo.toml Cargo.lock"
APP_PID=

stop_app() {
    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        kill "$APP_PID" 2>/dev/null
        # Give agl a moment to stop its agent processes.
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            kill -0 "$APP_PID" 2>/dev/null || break
            sleep 0.2
        done
        kill -9 "$APP_PID" 2>/dev/null
    fi
    APP_PID=
}

start_app() {
    # Launched from inside the bundle, so it keeps its name and Dock icon.
    "$APP_BIN" "$PROJECT" &
    APP_PID=$!
    echo "dev: running (pid $APP_PID) on $PROJECT"
}

build() {
    echo "dev: building…"
    if scripts/bundle.sh >/dev/null 2>target/.dev-build.log; then
        return 0
    fi
    grep -E "^(error|warning: unused)" -A12 target/.dev-build.log | head -60
    echo "dev: build failed; keeping the running app. Waiting for changes…"
    return 1
}

changed() {
    [ -n "$(find $WATCH -newer "$STAMP" -not -name '.*' 2>/dev/null | head -1)" ]
}

trap 'stop_app; exit 0' INT TERM HUP
trap stop_app EXIT

mkdir -p target
touch "$STAMP"
build && start_app
echo "dev: watching $WATCH (Ctrl-C to quit)"

while true; do
    sleep 1
    if ! changed; then
        # Restart the app if it was closed, but not after a failed build.
        if [ -n "$APP_PID" ] && ! kill -0 "$APP_PID" 2>/dev/null; then
            echo "dev: app exited; waiting for changes…"
            APP_PID=
        fi
        continue
    fi
    # Let a burst of saves settle before building.
    sleep 0.3
    touch "$STAMP"
    if build; then
        stop_app
        start_app
    fi
done
