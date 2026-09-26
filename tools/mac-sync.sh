#!/usr/bin/env bash
# tools/mac-sync.sh
# Run on macOS to capture gibberish daemon logs, dongle serial info, and Slint screenshots,
# then upload them directly to the Linux debugging sink.

set -euo pipefail

LINUX_IP="${1:-10.0.0.12}"
PORT="${2:-4484}"
UPLOAD_URL="http://${LINUX_IP}:${PORT}/upload"
STATUS_URL="http://${LINUX_IP}:${PORT}/status"

TMP_DIR="/tmp/gibberish"
mkdir -p "${TMP_DIR}"

SCREENSHOT_PATH="${TMP_DIR}/mac_client_window.png"
SERIAL_INFO_PATH="${TMP_DIR}/mac_dongle_info.txt"
DAEMON_LOG_PATH="${TMP_DIR}/daemon.log"

echo "=========================================================="
echo "🍏 Gibberish macOS Diagnostic & Screenshot Sync Tool"
echo "Target Linux Host: ${UPLOAD_URL}"
echo "=========================================================="

# 1. Check network connectivity to Linux sink
echo -n "🔍 Checking connectivity to Linux debug sink... "
if curl -s --connect-timeout 3 "${STATUS_URL}" > /dev/null; then
    echo "CONNECTED ✅"
else
    echo "FAILED ❌"
    echo "Error: Could not reach Linux sink at ${STATUS_URL}."
    echo "Make sure tools/remote-debug-sink.py is running on Linux (IP: ${LINUX_IP})."
    exit 1
fi

# 2. Probe connected USB dongles on macOS
echo "🔌 Probing USB-CDC dongle interfaces..."
{
    echo "=== macOS Serial Ports ==="
    ls -la /dev/cu.usbmodem* /dev/tty.usbmodem* 2>/dev/null || echo "No /dev/*.usbmodem devices found."
    echo ""
    echo "=== USB System Profiler (LilyGO / ESP32-C5) ==="
    system_profiler SPUSBDataType 2>/dev/null | grep -A 8 -i "USB JTAG/serial" || echo "No USB JTAG device matched in profiler."
    echo ""
    echo "=== Timestamp ==="
    date -u +"%Y-%m-%dT%H:%M:%SZ"
} > "${SERIAL_INFO_PATH}"
cat "${SERIAL_INFO_PATH}"

# 3. Capture Slint Client Screenshot
echo "📸 Capturing UI screenshot..."
# Look for a window named Gibberish or Slint, or fallback to interactive/desktop capture
if pgrep -f "gibberish-client" > /dev/null; then
    echo "Gibberish client process detected (PID: $(pgrep -f gibberish-client))."
    # Bring client window to foreground on macOS
    osascript -e 'tell application "System Events" to set frontmost of (first process whose unix id is '$(pgrep -f gibberish-client | head -n1)') to true' 2>/dev/null || true
    sleep 0.5
    # Capture display without shadow
    screencapture -x -C "${SCREENSHOT_PATH}" || screencapture -x "${SCREENSHOT_PATH}"
else
    echo "Gibberish client process not found in pgrep. Capturing primary desktop..."
    screencapture -x "${SCREENSHOT_PATH}"
fi
echo "Screenshot saved: ${SCREENSHOT_PATH} ($(stat -f%z "${SCREENSHOT_PATH}" 2>/dev/null || stat -c%s "${SCREENSHOT_PATH}") bytes)"

# 4. Check for daemon log
if [ ! -f "${DAEMON_LOG_PATH}" ]; then
    echo "⚠️  No daemon log found at ${DAEMON_LOG_PATH}. Creating placeholder."
    echo "Daemon log not present at /tmp/gibberish/daemon.log at sync time." > "${DAEMON_LOG_PATH}"
fi

# 5. Upload artifacts to Linux sink
echo "🚀 Uploading diagnostics and screenshots to Linux host..."
curl -F "screenshot=@${SCREENSHOT_PATH}" \
     -F "serial_info=@${SERIAL_INFO_PATH}" \
     -F "daemon_log=@${DAEMON_LOG_PATH}" \
     "${UPLOAD_URL}"

echo ""
echo "🎉 Sync complete! Diagnostic artifacts uploaded to Linux /tmp/gibberish/remote_mac/"
