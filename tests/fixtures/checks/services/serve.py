"""Answers GET /health with 200 while the folder it runs in has a file
`healthy`, and with 503 while it has not."""

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


print("serving", flush=True)
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), Health).serve_forever()
