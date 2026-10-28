#!/usr/bin/env python3
"""Prepare scoped local fixtures; does not download or alter the system profile."""
import pathlib,json,hashlib,base64,os,sys
root=pathlib.Path.cwd();home=root/'.spike-home'
for d in ['downloads','logs','tmp','www','profile/NativeMessagingHosts']:(home/d).mkdir(parents=True,exist_ok=True)
os.chmod(home/'profile',0o700)
m=json.load(open(root/'spikes/ext/manifest.json'));key=base64.b64decode(m['key']);eid=''.join(chr(97+int(x,16)) for x in hashlib.sha256(key).hexdigest()[:32]);(home/'ext-id').write_text(eid)
manifest={'name':'org.iksa.spike','description':'feasibility echo','path':str(root/'spikes/native-host.py'),'type':'stdio','allowed_origins':['chrome-extension://'+eid+'/']}
(home/'profile/NativeMessagingHosts/org.iksa.spike.json').write_text(json.dumps(manifest,indent=2))
(home/'www/form.html').write_text('''<!doctype html><h1>Spike form</h1><input id=q><button id=go onclick="document.querySelector('#out').textContent=document.querySelector('#q').value">Go</button><output id=out></output>''')
print('extension ID:',eid)
print('native manifest:',home/'profile/NativeMessagingHosts/org.iksa.spike.json')
