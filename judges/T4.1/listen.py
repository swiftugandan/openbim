# TCP listener for the sandbox probe: prints its port, then records any accepted connection.
import socket, sys

out = sys.argv[1]
s = socket.socket()
s.bind(("127.0.0.1", 0))
s.listen(8)
s.settimeout(1.0)
print(s.getsockname()[1], flush=True)
while True:
    try:
        c, _ = s.accept()
    except socket.timeout:
        continue
    with open(out, "a") as f:
        f.write("CONNECTED\n")
    c.close()
