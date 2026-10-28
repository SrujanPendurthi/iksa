#!/usr/local/bin/python3
import sys,struct,json
n=struct.unpack("<I",sys.stdin.buffer.read(4))[0]
x=json.loads(sys.stdin.buffer.read(n))
b=json.dumps({"pong":x}).encode()
sys.stdout.buffer.write(struct.pack("<I",len(b))+b)
sys.stdout.buffer.flush()
