# Justfile for Project Gibberish

default:
    @just --list

# Start desktop daemon (auto-detects serial dongle if port not specified)
daemon port="" log="/tmp/gibberish/daemon.log":
    @mkdir -p /tmp/gibberish
    cargo run --release -p gibberish-daemon -- {{ if port != "" { "--port " + port } else { "" } }} --log-file {{log}}

# Start daemon in background and follow logs
daemon-bg port="" log="/tmp/gibberish/daemon.log":
    @mkdir -p /tmp/gibberish
    @pkill -f gibberish-daemon 2>/dev/null || true
    cargo run --release -p gibberish-daemon -- {{ if port != "" { "--port " + port } else { "" } }} --log-file {{log}} > /dev/null 2>&1 &
    @echo "gibberishd launched in background (logging to {{log}})"
    @sleep 1
    tail -f {{log}}

# Stop any running daemon instance
stop-daemon:
    @pkill -f gibberish-daemon 2>/dev/null && echo "Stopped running gibberish-daemon" || echo "No running daemon found"

# Tail live daemon & dongle logs
logs log="/tmp/gibberish/daemon.log":
    @mkdir -p /tmp/gibberish
    @touch {{log}}
    tail -f {{log}}

# Flash firmware to C5 dongle (auto-detects port if not specified)
flash port="":
    cd apps/gibberish-firmware && espflash flash {{ if port != "" { "--port " + port } else { "" } }} --release

# Build firmware with debug telemetry profile (5s unencrypted beacon)
build-debug:
    cd apps/gibberish-firmware && cargo build --release --features debug-telemetry

build-firmware-debug: build-debug

# Build firmware with production profile (60s stealth beacon)
build-prod:
    cd apps/gibberish-firmware && cargo build --release --no-default-features --features prod

build-firmware-prod: build-prod

# Flash debug firmware to C5 dongle (auto-detects port if not specified)
flash-debug port="":
    cd apps/gibberish-firmware && espflash flash {{ if port != "" { "--port " + port } else { "" } }} --release --features debug-telemetry

flash-firmware-debug port="": (flash-debug port)

# Flash production firmware to C5 dongle (auto-detects port if not specified)
flash-prod port="":
    cd apps/gibberish-firmware && espflash flash {{ if port != "" { "--port " + port } else { "" } }} --release --no-default-features --features prod

flash-firmware-prod port="": (flash-prod port)

# Run central companion fleet sink
sink:
    cargo run --release -p gibberish-daemon --bin gibberish-sink

# Run multi-node end-to-end telemetry verification
test-fleet:
    cargo run --release -p gibberish-daemon --bin fleet_sink_test

# Run host unit and integration tests
test-host:
    cargo test --workspace

# Run multi-node mesh swarm simulation
test-sim:
    cargo run -p integration-sim

# Check security guardrails (deny crypto in firmware)
check-deny:
    cargo deny check

# Render headless visual UI screenshots across states into docs/screenshots
screenshot:
    cargo test -p gibberish-client --test visual_snapshot_test
