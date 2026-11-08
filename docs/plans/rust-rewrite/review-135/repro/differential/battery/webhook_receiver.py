"""The Webhook receiver both servers post Notifications to.

A Webhook URL is a credential, so the two servers must not share one endpoint:
each gets its own, and the two recorded bodies are compared afterwards. The
receiver answers 200 to anything and appends one JSON line per delivery.
"""

from __future__ import annotations

import argparse
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


class Handler(BaseHTTPRequestHandler):
    delivery_log: Path

    def do_POST(self) -> None:  # noqa: N802 - http.server's spelling
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length).decode()
        try:
            parsed = json.loads(body)
        except ValueError:
            parsed = body
        with self.delivery_log.open("a") as handle:
            handle.write(json.dumps({"path": self.path, "body": parsed}) + "\n")
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, *args) -> None:
        pass


def serve(port: int, delivery_log: Path) -> ThreadingHTTPServer:
    handler = type("BoundHandler", (Handler,), {"delivery_log": delivery_log})
    server = ThreadingHTTPServer(("127.0.0.1", port), handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--py-port", type=int, default=18291)
    parser.add_argument("--rs-port", type=int, default=18292)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    out = Path(args.out)
    serve(args.py_port, out / "webhooks-py.jsonl")
    serve(args.rs_port, out / "webhooks-rs.jsonl")
    print(f"receivers on {args.py_port} and {args.rs_port}", flush=True)
    threading.Event().wait()


if __name__ == "__main__":
    main()
