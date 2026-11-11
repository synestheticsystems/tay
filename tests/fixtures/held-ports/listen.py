"""Listens on 127.0.0.1 at the port it is given, and holds each connection
it is offered open until it is stopped, saying so of each."""
import socket
import sys

server = socket.socket()
server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
server.bind(("127.0.0.1", int(sys.argv[1])))
server.listen(8)
print("listening", flush=True)
held = []
while True:
    connection, _ = server.accept()
    held.append(connection)
    print("holding a connection", flush=True)
