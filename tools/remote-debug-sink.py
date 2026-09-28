#!/usr/bin/env python3
"""
Remote Debug Sink for Gibberish Cross-Machine Testing
Listens on 0.0.0.0:4484 to collect logs, screenshots, and telemetry from remote test nodes (e.g. macOS).
Saves received artifacts to /tmp/gibberish/remote_mac/
Compatible with Python 3.8 - 3.13+ (no deprecated cgi module).
"""

import email
from email import policy
import http.server
import json
import os
import socketserver
import sys
from datetime import datetime
from pathlib import Path

PORT = 4484
UPLOAD_DIR = Path("/tmp/gibberish/remote_mac")

class RemoteDebugHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        now = datetime.now().strftime("%Y-%m-%d %H:%M:%S")
        sys.stderr.write(f"[{now}] [RemoteDebugSink] {format % args}\n")

    def do_GET(self):
        if self.path == "/status" or self.path == "/":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            artifacts = [p.name for p in UPLOAD_DIR.glob("*") if p.is_file()] if UPLOAD_DIR.exists() else []
            resp = {
                "status": "ready",
                "service": "gibberish-debug-sink",
                "upload_dir": str(UPLOAD_DIR),
                "artifacts": artifacts
            }
            self.wfile.write(json.dumps(resp, indent=2).encode("utf-8"))
        else:
            super().do_GET()

    def do_POST(self):
        if self.path == "/upload":
            UPLOAD_DIR.mkdir(parents=True, exist_ok=True)
            content_type = self.headers.get("Content-Type", "")
            content_len = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_len)

            if "multipart/form-data" in content_type:
                # Wrap body in MIME header for email parser
                msg_bytes = f"Content-Type: {content_type}\r\nMIME-Version: 1.0\r\n\r\n".encode("latin1") + body
                msg = email.message_from_bytes(msg_bytes, policy=policy.default)

                saved_files = []
                for part in msg.iter_parts():
                    filename = part.get_filename()
                    if filename:
                        safe_filename = Path(filename).name
                        target_path = UPLOAD_DIR / safe_filename
                        payload = part.get_payload(decode=True)
                        if payload is not None:
                            with open(target_path, "wb") as f:
                                f.write(payload)
                            saved_files.append(safe_filename)
                            print(f"📥 Received file: {safe_filename} ({len(payload)} bytes)")
                    else:
                        name = part.get_param("name", header="content-disposition")
                        text_payload = part.get_payload(decode=True)
                        if text_payload:
                            print(f"📝 Received field {name}: {text_payload[:120]}")

                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({
                    "status": "ok",
                    "saved_files": saved_files,
                    "target_dir": str(UPLOAD_DIR)
                }).encode("utf-8"))
            elif "application/json" in content_type:
                data = json.loads(body.decode("utf-8"))
                timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
                target_path = UPLOAD_DIR / f"telemetry_{timestamp}.json"
                with open(target_path, "w") as f:
                    json.dump(data, f, indent=2)
                print(f"📥 Saved telemetry JSON: {target_path}")

                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"status": "ok", "saved": str(target_path)}).encode("utf-8"))
            else:
                self.send_response(400)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"error": "Unsupported Content-Type"}')
        else:
            self.send_response(404)
            self.end_headers()

def main():
    UPLOAD_DIR.mkdir(parents=True, exist_ok=True)
    socketserver.TCPServer.allow_reuse_address = True
    print(f"=======================================================")
    print(f"🚀 Gibberish Remote Debug Sink listening on 0.0.0.0:{PORT}")
    print(f"📁 Destination directory: {UPLOAD_DIR}")
    print(f"📡 Mac command: ./tools/mac-sync.sh <linux_ip>")
    print(f"=======================================================")
    with socketserver.TCPServer(("0.0.0.0", PORT), RemoteDebugHandler) as httpd:
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nShutting down sink server.")

if __name__ == "__main__":
    main()
