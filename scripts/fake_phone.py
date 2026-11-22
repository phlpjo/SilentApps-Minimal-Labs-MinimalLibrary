# Nachgebautes Handy (Minimal Player Connected, Empfang) für: cargo run -p musiccore --example wlan_selftest
# Speichert nach /tmp/wlt/phone, Code 123456
import socket, threading, json, os, http.server, urllib.parse, shutil
ROOT='/tmp/wlt/phone'; os.makedirs(ROOT, exist_ok=True)
TOK='abcdef0123456789abcdef0123456789abcdef0123456789'
LOG=[]
def udp():
    s=socket.socket(socket.AF_INET, socket.SOCK_DGRAM); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR,1); s.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST,1)
    s.bind(('',47820))
    while True:
        d,a=s.recvfrom(512)
        if d.startswith(b'MLDISCOVER1'):
            s.sendto(json.dumps({"id":"fake1","name":"Testhandy","port":47821,"ready":True,"free":5_000_000_000}).encode(),a)
threading.Thread(target=udp,daemon=True).start()
class H(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def body(self):
        if 'chunked' in (self.headers.get('Transfer-Encoding') or ''):
            out=b''
            while True:
                n=int(self.rfile.readline().strip().split(b';')[0],16)
                if n==0: self.rfile.readline(); return out
                out+=self.rfile.read(n); self.rfile.readline()
        return self.rfile.read(int(self.headers.get('Content-Length') or 0))
    def send(self,code,b=b''):
        self.send_response(code); self.send_header('Content-Length',str(len(b))); self.send_header('Connection','close'); self.end_headers(); self.wfile.write(b); self.close_connection=True
    def auth(self): return self.headers.get('X-Token')==TOK
    def rel(self): return urllib.parse.unquote(self.path[3:])
    def do_POST(self):
        b=self.body(); LOG.append((self.command,self.path))
        if self.path=='/pair':
            return self.send(200,json.dumps({"token":TOK}).encode()) if b==b'123456' else self.send(403)
        if not self.auth(): return self.send(401)
        if self.path=='/done': open('/tmp/wlt/done.json','wb').write(b)
        self.send(200)
    def do_GET(self):
        if not self.auth(): return self.send(401)
        if self.path=='/list':
            out=''
            for d,_,fs in os.walk(ROOT):
                for f in fs:
                    p=os.path.join(d,f); out+=f"{os.path.getsize(p)}\t{os.path.relpath(p,ROOT)}\n"
            return self.send(200,out.encode())
        self.send(404)
    def do_PUT(self):
        if not self.auth(): return self.send(401)
        b=self.body(); p=os.path.join(ROOT,self.rel()); os.makedirs(os.path.dirname(p),exist_ok=True); open(p,'wb').write(b); self.send(200)
    def do_DELETE(self):
        if not self.auth(): return self.send(401)
        p=os.path.join(ROOT,self.rel())
        if os.path.exists(p): os.remove(p); self.send(200)
        else: self.send(404)
    def log_message(self,*a): pass
http.server.ThreadingHTTPServer(('',47821),H).serve_forever()
