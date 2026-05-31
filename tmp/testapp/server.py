from http.server import HTTPServer, BaseHTTPRequestHandler

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"Hello from PaaSTech!\n")

HTTPServer(("0.0.0.0", int(__import__('os').environ.get('PORT', 8080))), Handler).serve_forever()
