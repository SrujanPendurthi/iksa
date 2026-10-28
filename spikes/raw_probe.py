#!/usr/bin/env python3
"""Throwaway stdlib-only CDP websocket, to separate protocol from crate behavior."""
import socket,base64,os,hashlib,struct,json,time,subprocess,pathlib,urllib.request,sys,shlex,re,statistics,select
ROOT=pathlib.Path.cwd(); HOME=ROOT/'.spike-home'
class CDP:
 def __init__(self,url):
  from urllib.parse import urlparse
  u=urlparse(url);self.s=socket.create_connection((u.hostname,u.port),timeout=30);key=base64.b64encode(os.urandom(16)).decode();self.s.sendall(f'GET {u.path} HTTP/1.1\r\nHost: {u.hostname}:{u.port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n'.encode());b=b''
  while not b.endswith(b'\r\n\r\n'):b+=self.s.recv(1)
  assert b'101' in b,b
  self.i=0;self.events=[];self.acks=set();self.ack_responses=0
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
  message=json.loads(v)
  if message.get('method')=='Page.screencastFrame':
   # Send only: a recursive call would consume the outer request's reply.
   ack=self.send('Page.screencastFrameAck',{'sessionId':message['params']['sessionId']},message.get('sessionId'))
   self.acks.add(ack)
  if message.get('id') in self.acks:
   self.acks.remove(message['id']);self.ack_responses+=1
   if 'error' in message:raise RuntimeError('screencast ack: '+json.dumps(message['error']))
   return {}
  return message
 def send(self,m,p=None,session=None):
  self.i+=1;d={'id':self.i,'method':m,'params':p or {}}
  if session:d['sessionId']=session
  b=json.dumps(d).encode();n=len(b);mask=os.urandom(4);h=bytes([129,128|(n if n<126 else 126)])+(struct.pack('!H',n) if n>=126 else b'');self.s.sendall(h+mask+bytes(x^mask[i%4] for i,x in enumerate(b)))
  return self.i
 def call(self,m,p=None,session=None):
  request_id=self.send(m,p,session)
  while True:
   r=self.frame()
   if r.get('id')==request_id:
    if 'error' in r:raise RuntimeError(json.dumps(r['error']))
    return r.get('result',{})
   if r:self.events.append(r)
def main():
 mode=sys.argv[1]
 if mode=='s3':return s3_run()
 if mode=='s5' and not os.environ.get('SPIKE_S5_CHILD'):
  return s5_runs()
 profile=HOME/(('profile-raw-'+mode+'-'+str(time.time_ns())) if mode in ('s5','s2') else ('profile-raw-'+mode));profile.mkdir(mode=0o700,exist_ok=True);(profile/'DevToolsActivePort').unlink(missing_ok=True)
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
   s2_experiment(c,ctx)
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
 except Exception as e:print('BLOCKED s2' if mode=='s2' else 'ERROR',type(e).__name__+': '+str(e),flush=True);raise
 finally:
  try:proc.wait(timeout=5)
  except subprocess.TimeoutExpired:proc.terminate();proc.wait(timeout=5)
def s2_experiment(c,ctx):
 viewport={'width':1280,'height':800,'deviceScaleFactor':1,'mobile':False}
 url='http://127.0.0.1:8765/form.html'
 def evaluate(session,expression):
  result=c.call('Runtime.evaluate',{'expression':expression,'returnByValue':True},session)
  if 'exceptionDetails' in result:raise RuntimeError(json.dumps(result['exceptionDetails']))
  return result['result'].get('value')
 def prepare(hidden):
  params={'url':'about:blank','browserContextId':ctx}
  if hidden:params['hidden']=True
  target=c.call('Target.createTarget',params)['targetId']
  print(('hidden' if hidden else 'visible')+'_target='+target,flush=True)
  try:print(('hidden' if hidden else 'visible')+'_window',c.call('Browser.getWindowForTarget',{'targetId':target}),flush=True)
  except RuntimeError as exc:print('hidden_window_error='+str(exc),flush=True)
  session=c.call('Target.attachToTarget',{'targetId':target,'flatten':True})['sessionId']
  c.call('Page.enable',{},session)
  c.call('Emulation.setDeviceMetricsOverride',viewport,session)
  print('navigate',c.call('Page.navigate',{'url':url},session),flush=True)
  deadline=time.monotonic()+10
  while not evaluate(session,"document.readyState==='complete' && !!document.querySelector('#go')"):
   if time.monotonic()>deadline:raise TimeoutError('fixture not ready within 10s')
   time.sleep(.05)
  evaluate(session,"window.spikeTick=0;setInterval(()=>{document.body.style.background=(++window.spikeTick%2)?'red':'blue'},100);''")
  print('viewport',evaluate(session,'JSON.stringify({width:innerWidth,height:innerHeight,url:location.href})'),flush=True)
  return session
 def sample(session,label):
  # Receive throughout the sample: sleeping here would exhaust the frame window.
  c.call('Page.startScreencast',{},session)
  start=time.monotonic();deadline=start+6;count=0
  while time.monotonic()<deadline:
   if not select.select([c.s],[],[],max(0,deadline-time.monotonic()))[0]:break
   event=c.frame()
   if event.get('method')=='Page.screencastFrame' and event.get('sessionId')==session:count+=1
  c.call('Page.stopScreencast',{},session)
  print(label+'='+str(count),flush=True)
  print(label+'_sample_seconds='+str(time.monotonic()-start),flush=True)
  return count
 hidden=prepare(True)
 frames=sample(hidden,'hidden_frames_viewport')
 if not frames:
  c.call('Emulation.setFocusEmulationEnabled',{'enabled':True},hidden)
  sample(hidden,'hidden_frames_viewport_focus')
 evaluate(hidden,"document.querySelector('#q').focus();''")
 c.call('Input.insertText',{'text':'forwarded'},hidden)
 point=evaluate(hidden,"(()=>{const r=document.querySelector('#go').getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()")
 print('forwarded_click_center='+json.dumps(point),flush=True)
 for kind in ['mousePressed','mouseReleased']:
  c.call('Input.dispatchMouseEvent',{'type':kind,'button':'left','clickCount':1,**point},hidden)
 print('forwarded_out='+str(evaluate(hidden,"document.querySelector('#out').textContent")),flush=True)
 visible=prepare(False)
 sample(visible,'visible_control_frames')
 print('ack_responses='+str(c.ack_responses)+' pending_acks='+str(len(c.acks)),flush=True)

def netlog_hosts(path):
 """Decode numeric constants and scan request/resolver/socket event and source params."""
 import ipaddress
 from urllib.parse import urlsplit
 data=json.loads(path.read_text());constants=data['constants']
 maps={key:{str(v):k for k,v in constants[key].items()} for key in ['logEventTypes','logSourceType']}
 hosts={};excluded=set();scope={};fields={};all_counts={}
 def add(value,event,field):
  value=value.strip()
  if not value:return
  if '://' in value:
   u=urlsplit(value)
   if u.scheme not in ('http','https','ws','wss','ftp'):
    excluded.add(value);return
   host=u.hostname
  else:
   if value.startswith('['):host=value[1:].split(']')[0]
   elif value.count(':')==1:host=value.split(':')[0]
   else:host=value
  if not host:return
  host=host.lower().rstrip('.')
  try:loopback=ipaddress.ip_address(host.split('%')[0]).is_loopback
  except ValueError:loopback=host=='localhost' or host.endswith('.localhost')
  hosts.setdefault(host,{'loopback':loopback,'evidence':[]})['evidence'].append([event,field,value])
 def walk(value,event,field=''):
  if isinstance(value,dict):
   for k,v in value.items():walk(v,event,field+'.'+k)
  elif isinstance(value,list):
   for v in value:walk(v,event,field+'[]')
  elif isinstance(value,str):
   fields.setdefault(event,{}).setdefault(field,set()).add(value)
   key=field.lower()
   if '://' in value or any(x in key for x in ['host','address','endpoint','remote_peer','ip_endpoint','alias','destination']):
    # Resolver diagnostic strings such as address_family are not hosts.
    if '://' in value or re.fullmatch(r'(?:\[[0-9a-fA-F:%]+\](?::[0-9]+)?|[0-9a-fA-F:]+|[A-Za-z0-9_.-]+(?::[0-9]+)?)',value):add(value,event,field)
 for e in data['events']:
  event=maps['logEventTypes'].get(str(e['type']),str(e['type']))
  source=maps['logSourceType'].get(str(e.get('source',{}).get('type')),str(e.get('source',{}).get('type')))
  all_label=event+' / '+source;all_counts[all_label]=all_counts.get(all_label,0)+1
  if event=='URL_REQUEST_START_JOB' or any(x in event or x in source for x in ['SOCKET','HOST_RESOLVER']):
   label=event+' / '+source;scope[label]=scope.get(label,0)+1;walk(e.get('params',{}),label)
 audit={'constants':maps,'all_event_source_counts':all_counts,'event_source_counts':scope,'string_params':{e:{k:sorted(v) for k,v in f.items()} for e,f in fields.items()},'hosts':hosts,'excluded_non_network_urls':sorted(excluded),'total_events':len(data['events'])}
 audit_path=path.with_suffix('.audit.json');audit_path.write_text(json.dumps(audit,indent=2))
 print('NETLOG_EVENTS',len(data['events']),'SELECTED_EVENTS',sum(scope.values()),'AUDIT',str(audit_path),flush=True)
 print('EVENT_SOURCE_COUNTS',json.dumps(scope,sort_keys=True),flush=True)
 print('EXCLUDED_NON_NETWORK_URLS',json.dumps(sorted(excluded)),flush=True)
 for host,info in sorted(hosts.items()):print('HOST',host,'LOOPBACK='+str(info['loopback']),'EVIDENCE='+json.dumps(info['evidence']),flush=True)
 print('HOST_LIST='+json.dumps(sorted(hosts)),flush=True)
 print('NON_LOOPBACK='+str(sum(not info['loopback'] for info in hosts.values())),flush=True)
 return audit

def s3_run():
 variant=os.environ.get('SPIKE_S3_VARIANT','noext')
 if variant not in ('noext','ext'):raise ValueError('SPIKE_S3_VARIANT must be noext or ext')
 stamp=str(time.time_ns());profile=HOME/('profile-raw-s3-'+variant+'-'+stamp);profile.mkdir(mode=0o700,parents=True)
 (HOME/'tmp').mkdir(exist_ok=True);(HOME/'logs').mkdir(exist_ok=True)
 path=HOME/'logs'/('netlog-s3-'+variant+'-'+stamp+'.json')
 args=['use-mock-keychain','remote-debugging-address=127.0.0.1','remote-debugging-port=0','disable-background-networking','disable-component-update','disable-sync','no-pings','disable-domain-reliability','disable-field-trial-config','disable-breakpad','metrics-recording-only','no-default-browser-check','no-first-run','safebrowsing-disable-auto-update','log-net-log='+str(path),'net-log-capture-mode=Default','user-data-dir='+str(profile)]
 if variant=='ext':args+=['load-extension='+str(ROOT/'spikes/ext')]
 env=os.environ|{'HOME':str(HOME),'CFFIXED_USER_HOME':str(HOME),'TMPDIR':str(HOME/'tmp')}
 print('VARIANT',variant,'PROFILE',str(profile),'NETLOG',str(path),'launch_args',json.dumps(args),flush=True)
 proc=None;c=None
 try:
  with path.with_suffix('.browser.log').open('w') as log:
   proc=subprocess.Popen([str(HOME/'chromium/Chromium.app/Contents/MacOS/Chromium'),*['--'+x for x in args],'about:blank'],env=env,stdout=log,stderr=log)
   for _ in range(200):
    if proc.poll() is not None:raise RuntimeError('browser exited during startup: '+str(proc.returncode))
    if (profile/'DevToolsActivePort').exists():break
    time.sleep(.1)
   else:raise TimeoutError('DevToolsActivePort not created within 20s')
   port,ws=(profile/'DevToolsActivePort').read_text().splitlines();c=CDP('ws://127.0.0.1:'+port+ws)
   print('version',c.call('Browser.getVersion'),flush=True)
   print('idle_start_targets',json.dumps(c.call('Target.getTargets')),flush=True)
   start=time.monotonic();time.sleep(120);elapsed=time.monotonic()-start
   print('IDLE_SECONDS='+str(elapsed),flush=True)
   print('idle_end_targets',json.dumps(c.call('Target.getTargets')),flush=True)
   c.call('Browser.close');proc.wait(timeout=15)
   if proc.returncode:raise RuntimeError('Browser.close exit code '+str(proc.returncode))
   print('BROWSER_CLOSE=clean EXIT=0 NETLOG_BYTES='+str(path.stat().st_size),flush=True)
  netlog_hosts(path)
  if variant=='ext':
   prefs=json.loads((profile/'Default/Secure Preferences').read_text())
   extension=prefs['extensions']['settings']['dmoglpnbfpfncbfhnbjokgfiecjofdcm']
   proof={k:extension.get(k) for k in ['path','state','disable_reasons','manifest','preferences']}
   print('EXTENSION_PERSISTED',json.dumps(proof),flush=True)
   if extension.get('disable_reasons') or pathlib.Path(extension['path']).resolve()!=(ROOT/'spikes/ext').resolve():raise RuntimeError('extension not enabled at expected path')
 except Exception as exc:
  print('BLOCKED s3 '+type(exc).__name__+': '+str(exc),flush=True);raise
 finally:
  if c:c.s.close()
  if proc and proc.poll() is None:
   proc.terminate()
   try:proc.wait(timeout=5)
   except subprocess.TimeoutExpired:proc.kill();proc.wait()

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
