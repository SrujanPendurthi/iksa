#!/usr/bin/env python3
"""Throwaway stdlib-only CDP websocket, to separate protocol from crate behavior."""
import socket,base64,os,hashlib,struct,json,time,subprocess,pathlib,urllib.request,sys,shlex,re,statistics
ROOT=pathlib.Path.cwd(); HOME=ROOT/'.spike-home'
class CDP:
 def __init__(self,url):
  from urllib.parse import urlparse
  u=urlparse(url);self.s=socket.create_connection((u.hostname,u.port),timeout=30);key=base64.b64encode(os.urandom(16)).decode();self.s.sendall(f'GET {u.path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode());b=b''
  while not b.endswith(b'\r\n\r\n'):b+=self.s.recv(1)
  assert b'101' in b,b
  self.i=0;self.events=[]
 def recv(self,n):
  b=b''
  while len(b)<n:
   x=self.s.recv(n-len(b))
   if not x:raise EOFError()
   b+=x
  return b
 def frame(self):
  a,b=self.recv(2);n=b&127
  if n==126:n=struct.unpack('!H',self.recv(2))[0]
  elif n==127:n=struct.unpack('!Q',self.recv(8))[0]
  mask=self.recv(4) if b&128 else None;v=self.recv(n)
  if mask:v=bytes(x^mask[i%4] for i,x in enumerate(v))
  return json.loads(v)
 def call(self,m,p=None,session=None):
  self.i+=1;d={'id':self.i,'method':m,'params':p or {}}
  if session:d['sessionId']=session
  b=json.dumps(d).encode();n=len(b);mask=os.urandom(4);h=bytes([129,128|(n if n<126 else 126)])+(struct.pack('!H',n) if n>=126 else b'');self.s.sendall(h+mask+bytes(x^mask[i%4] for i,x in enumerate(b)))
  while True:
   r=self.frame()
   if r.get('id')==self.i:
    if 'error' in r:raise RuntimeError(json.dumps(r['error']))
    return r.get('result',{})
   self.events.append(r)
def main():
 mode=sys.argv[1]
 if mode=='s5' and not os.environ.get('SPIKE_S5_CHILD'):
  return s5_runs()
 profile=HOME/(('profile-raw-'+mode+'-'+str(time.time_ns())) if mode=='s5' else ('profile-raw-'+mode));profile.mkdir(mode=0o700,exist_ok=True);(profile/'DevToolsActivePort').unlink(missing_ok=True)
 args=['use-mock-keychain','remote-debugging-address=127.0.0.1','remote-debugging-port=0','disable-background-networking','disable-component-update','disable-sync','no-pings','disable-domain-reliability','disable-field-trial-config','disable-breakpad','metrics-recording-only','no-default-browser-check','no-first-run','safebrowsing-disable-auto-update']
 if mode=='s5':
  extras=shlex.split(os.environ.get('SPIKE_ARGS',''))
  if any(x.startswith('-') or not x for x in extras):raise ValueError('SPIKE_ARGS requires space-separated bare keys')
  args+=extras
  print('launch_args',json.dumps(args), 'profile',str(profile),flush=True)
 env=os.environ|{'HOME':str(HOME),'CFFIXED_USER_HOME':str(HOME),'TMPDIR':str(HOME/'tmp')}
 log=open(HOME/'logs'/('s5-'+os.environ.get('SPIKE_CONFIG','custom')+'-'+os.environ.get('SPIKE_RUN','1')+'-browser.log' if mode=='s5' else 'raw-'+mode+'-browser.log'),'w');proc=subprocess.Popen([str(HOME/'chromium/Chromium.app/Contents/MacOS/Chromium'),*['--'+x for x in args],'--user-data-dir='+str(profile),'about:blank'],env=env,stdout=log,stderr=log)
 try:
  for _ in range(100):
   try:port,path=(profile/'DevToolsActivePort').read_text().splitlines();break
   except FileNotFoundError:time.sleep(.1)
  c=CDP('ws://127.0.0.1:'+port+path);print('version',c.call('Browser.getVersion'),flush=True)
  ctx=c.call('Target.createBrowserContext')['browserContextId']
  if mode=='s2':
   tid=c.call('Target.createTarget',{'url':'about:blank','hidden':True,'browserContextId':ctx})['targetId'];print('hidden_target',tid,flush=True)
   try:print('hidden_window',c.call('Browser.getWindowForTarget',{'targetId':tid}),flush=True)
   except Exception as e:print('hidden_window_error',str(e),flush=True)
   s=c.call('Target.attachToTarget',{'targetId':tid,'flatten':True})['sessionId'];c.call('Page.enable',session=s);print('navigate',c.call('Page.navigate',{'url':'http://127.0.0.1:8765/form.html'},s),flush=True);time.sleep(1)
   def js(x):return c.call('Runtime.evaluate',{'expression':x,'returnByValue':True},s)
   print('content',js('document.documentElement.outerHTML'),flush=True)
   print('focus',js("document.querySelector('#q').focus();''"),flush=True);c.call('Input.insertText',{'text':'raw typed'},s);print('click',js("document.querySelector('#go').click();document.querySelector('#out').textContent"),flush=True)
   c.call('Page.startScreencast',{},s);js("setInterval(()=>document.body.style.background=Math.random()>.5?'red':'blue',100);''");time.sleep(6);js('document.title');print('screencast_frames',sum(x.get('method')=='Page.screencastFrame' and x.get('sessionId')==s for x in c.events),flush=True)
   try:print('activate',c.call('Target.activateTarget',{'targetId':tid}),flush=True)
   except Exception as e:print('activate_error',str(e),flush=True)
   try:print('activated_window',c.call('Browser.getWindowForTarget',{'targetId':tid}),flush=True)
   except Exception as e:print('activated_window_error',str(e),flush=True)
   js("document.cookie='spike=same-context;path=/';document.cookie")
   v=c.call('Target.createTarget',{'url':'http://127.0.0.1:8765/form.html','browserContextId':ctx})['targetId'];vs=c.call('Target.attachToTarget',{'targetId':v,'flatten':True})['sessionId'];time.sleep(1);print('handoff_window',c.call('Browser.getWindowForTarget',{'targetId':v}),flush=True);print('handoff_cookie',c.call('Runtime.evaluate',{'expression':'document.cookie','returnByValue':True},vs),flush=True);c.call('Page.enable',{},vs);c.call('Page.startScreencast',{},vs);c.call('Runtime.evaluate',{'expression':"setInterval(()=>document.body.style.background=Math.random()>.5?'red':'blue',100);''"},vs);time.sleep(6);c.call('Runtime.evaluate',{'expression':'document.title'},vs);visible_frames=[e for e in c.events if e.get('method')=='Page.screencastFrame' and e.get('sessionId')==vs];print('visible_control_frames',len(visible_frames),flush=True)
   for e in visible_frames:c.call('Page.screencastFrameAck',{'sessionId':e['params']['sessionId']},vs)
  if mode=='s5':
   time.sleep(10);print('idle_targets',json.dumps(c.call('Target.getTargets')),flush=True);print(subprocess.check_output([sys.executable,'spikes/measure.py','idle',str(proc.pid)],text=True),flush=True)
   t=c.call('Target.createTarget',{'url':'about:blank','hidden':True,'browserContextId':ctx})['targetId'];s=c.call('Target.attachToTarget',{'targetId':t,'flatten':True})['sessionId'];c.call('Page.enable',{},s);print('agent_navigation',c.call('Page.navigate',{'url':'https://en.wikipedia.org/wiki/Rust_(programming_language)'},s),flush=True);time.sleep(15);print('loaded_targets',json.dumps(c.call('Target.getTargets')),flush=True);print('agent_content',c.call('Runtime.evaluate',{'expression':'JSON.stringify({title:document.title,bytes:document.documentElement.outerHTML.length})','returnByValue':True},s),flush=True);print(subprocess.check_output([sys.executable,'spikes/measure.py','agent',str(proc.pid)],text=True),flush=True)
  if mode=='s6':
   # No discoverTargets/autoAttach or target sessions during visible control load.
   for label,params in [('visible',{'url':'https://nowsecure.nl'}),('hidden',{'url':'about:blank','hidden':True,'browserContextId':ctx})]:
    t=c.call('Target.createTarget',params)['targetId'];print(label,'created',flush=True)
    if label=='visible':time.sleep(30)
    s=c.call('Target.attachToTarget',{'targetId':t,'flatten':True})['sessionId'];c.call('Network.enable',{},s)
    if label=='hidden':c.call('Page.enable',{},s);print('hidden navigate',c.call('Page.navigate',{'url':'https://nowsecure.nl'},s),flush=True);time.sleep(30)
    r=c.call('Runtime.evaluate',{'expression':'JSON.stringify({title:document.title,url:location.href,html:document.documentElement.outerHTML})','returnByValue':True},s);data=json.loads(r['result']['value']);(HOME/'logs'/('s6-'+label+'.html')).write_text(data['html']);h=data['html'];print(label,{'title':data['title'],'url':data['url'],'bytes':len(h),'Just a moment':'Just a moment' in h,'turnstile':'challenges.cloudflare.com/turnstile' in h or 'class="cf-turnstile"' in h,'hcaptcha':'hcaptcha.com/' in h or 'class="h-captcha"' in h,'recaptcha':'google.com/recaptcha' in h or 'class="g-recaptcha"' in h},flush=True)
    # Capture response status/header by explicit reload after observation.
    c.call('Page.reload',{},s);time.sleep(10);c.call('Runtime.evaluate',{'expression':'document.title'},s)
    for e in c.events:
     if e.get('method')=='Network.responseReceived' and e.get('sessionId')==s and e['params'].get('type')=='Document':
      z=e['params']['response'];print(label,'response',z['url'],z['status'],z['headers'],flush=True)
  c.call('Browser.close')
 except Exception as e:print('ERROR',repr(e),flush=True);raise
 finally:
  try:proc.wait(timeout=5)
  except subprocess.TimeoutExpired:proc.terminate();proc.wait(timeout=5)
def s5_runs():
 config=os.environ.get('SPIKE_CONFIG','custom')
 if not re.fullmatch(r'[A-Za-z0-9_-]+',config):raise ValueError('unsafe SPIKE_CONFIG')
 samples={'idle':[], 'agent':[], 'idle_rss':[]}
 for run in range(1,4):
  path=HOME/'logs'/f's5-{config}-{run}.log'
  print('RUN',config,run,str(path),flush=True)
  with path.open('w') as log:
   result=subprocess.run([sys.executable,__file__,'s5'],env=os.environ|{'SPIKE_S5_CHILD':'1','SPIKE_RUN':str(run)},stdout=log,stderr=subprocess.STDOUT)
  data=path.read_text()
  if result.returncode:raise RuntimeError(f'{path}: child exit {result.returncode}')
  for label in ['idle','agent']:
   match=re.search(r'^'+label+r' FOOTPRINT_KiB=([\d.]+)$',data,re.M)
   if match:samples[label].append(float(match[1]))
   else:
    causes=[line for line in data.splitlines() if 'BLOCKED' in line]
    print('BLOCKED',config,run,label,'log='+str(path),'cause='+repr(causes or ['FOOTPRINT_KiB line missing']),flush=True)
  match=re.search(r'^idle .* RSS_KiB=([\d.]+)$',data,re.M)
  if match:samples['idle_rss'].append(float(match[1]))
 for label,values in samples.items():
  print('MEDIAN',config,label,'KiB='+str(statistics.median(values)) if len(values)==3 else 'BLOCKED', 'samples='+str(values),flush=True)
if __name__=='__main__':main()
