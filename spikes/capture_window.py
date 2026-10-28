#!/usr/bin/env python3
"""Capture only a window owned by the exact scoped Chromium executable PID."""
import ctypes,plistlib,subprocess,pathlib,json
root=pathlib.Path.cwd();prefix=str(root/'.spike-home/chromium/Chromium.app/Contents/MacOS/Chromium')+' '
rows=subprocess.check_output(['ps','-axo','pid=,command='],text=True).splitlines()
pids=[int(r.strip().split(None,1)[0]) for r in rows if len(r.strip().split(None,1))==2 and r.strip().split(None,1)[1].startswith(prefix)]
assert len(pids)==1,('expected exactly one scoped Chromium',pids)
CG=ctypes.CDLL('/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics');CF=ctypes.CDLL('/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation')
CG.CGWindowListCopyWindowInfo.argtypes=[ctypes.c_uint32,ctypes.c_uint32];CG.CGWindowListCopyWindowInfo.restype=ctypes.c_void_p
CF.CFPropertyListCreateData.argtypes=[ctypes.c_void_p,ctypes.c_void_p,ctypes.c_long,ctypes.c_ulong,ctypes.c_void_p];CF.CFPropertyListCreateData.restype=ctypes.c_void_p
CF.CFDataGetLength.argtypes=[ctypes.c_void_p];CF.CFDataGetLength.restype=ctypes.c_long
CF.CFDataGetBytePtr.argtypes=[ctypes.c_void_p];CF.CFDataGetBytePtr.restype=ctypes.c_void_p
CF.CFRelease.argtypes=[ctypes.c_void_p]
a=CG.CGWindowListCopyWindowInfo(17,0);d=CF.CFPropertyListCreateData(None,a,100,0,None);windows=plistlib.loads(ctypes.string_at(CF.CFDataGetBytePtr(d),CF.CFDataGetLength(d)));CF.CFRelease(d);CF.CFRelease(a)
owned=[w for w in windows if w.get('kCGWindowOwnerPID')==pids[0] and w.get('kCGWindowLayer')==0]
print('scoped_windows',json.dumps(owned),flush=True)
assert owned,'No on-screen scoped Chromium windows available'
w=max(owned,key=lambda w:w['kCGWindowBounds']['Width']*w['kCGWindowBounds']['Height']);dest=root/'.spike-home/logs/s4-browser-window.png'
p=subprocess.run(['screencapture','-x','-o','-l',str(w['kCGWindowNumber']),str(dest)],capture_output=True,text=True);print('capture',p.returncode,p.stdout,p.stderr,flush=True)
if p.returncode:raise SystemExit(p.returncode)
