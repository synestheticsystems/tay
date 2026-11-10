"""Answers GET /health with 200 while the folder it runs in has a file
`healthy`, and with 503 while it has not, on the port it is given, at the
address it is given or else 127.0.0.1."""

import http.server
import os
import sys


class Health(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        status = 200 if os.path.exists("healthy") else 503
        self.send_response(status)
        self.end_headers()

    def log_message(self, format, *args):
        print(format % args, flush=True)


# A port written `@FILE` is read from that file.
port = sys.argv[1]
if port.startswith("@"):
    port = open(port[1:]).read().strip()
address = sys.argv[2] if len(sys.argv) > 2 else "127.0.0.1"
print("serving", flush=True)
http.server.HTTPServer((address, int(port)), Health).serve_forever()
