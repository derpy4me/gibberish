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

# Flash production firmware to C5 dongle (default release is production; auto-detects port if not specified)
flash port="":
    cd apps/gibberish-firmware && cargo build --release && espflash flash {{ if port != "" { "--port " + port } else { "" } }} target/riscv32imac-unknown-none-elf/release/gibberish-firmware

# Build firmware with serial debug output enabled (debug-telemetry feature; on-air beacon/telemetry cadence is the same as production)
build-debug:
    cd apps/gibberish-firmware && cargo build --release --features debug-telemetry

build-firmware-debug: build-debug

# Build production firmware (currently still broadcasts beacons and telemetry)
build-prod:
    cd apps/gibberish-firmware && cargo build --release

build-firmware-prod: build-prod

# Flash debug firmware to C5 dongle (auto-detects port if not specified)
flash-debug port="":
    cd apps/gibberish-firmware && cargo build --release --features debug-telemetry && espflash flash {{ if port != "" { "--port " + port } else { "" } }} target/riscv32imac-unknown-none-elf/release/gibberish-firmware

flash-firmware-debug port="": (flash-debug port)

# Flash production firmware to C5 dongle (auto-detects port if not specified)
flash-prod port="":
    cd apps/gibberish-firmware && cargo build --release && espflash flash {{ if port != "" { "--port " + port } else { "" } }} target/riscv32imac-unknown-none-elf/release/gibberish-firmware

flash-firmware-prod port="": (flash-prod port)

# Run central companion fleet sink (receives telemetry only from debug-telemetry firmware)
sink:
    cargo run --release -p gibberish-daemon --bin gibberish-sink

# Run telemetry framing/parser checks; probes /dev/ttyACM0 and /dev/ttyACM1 if attached
test-fleet:
    cargo run --release -p gibberish-daemon --bin fleet_sink_test

# Run host unit and integration tests
test-host:
    cargo test --workspace

# Run single-process crypto/chunk/storage round-trip simulation (no radio or mesh simulation)
test-sim:
    cargo run -p integration-sim

# Run over-the-air RF mesh test; requires two dongles on /dev/ttyACM0 and /dev/ttyACM1 with hard-coded node IDs
ota-mesh:
    cargo run --release -p gibberish-daemon --bin ota_mesh_test

# Run cargo-deny policy check at the root workspace (does not inspect firmware, which is excluded from it; not verified to pass)
check-deny:
    cargo deny check

# Render headless visual UI screenshots across states into docs/screenshots
screenshot:
    cargo test -p gibberish-client --test visual_snapshot_test
