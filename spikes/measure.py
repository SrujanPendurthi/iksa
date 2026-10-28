import subprocess,sys,os
rows=subprocess.check_output(['ps','-axo','pid=,ppid=,rss=,command='],text=True).splitlines()
procs={}
for row in rows:
 p,pp,rss,cmd=row.strip().split(None,3);procs[int(p)]=(int(pp),int(rss),cmd)
seeds={os.getppid()}
while True:
 extra={p for p,(pp,_,_) in procs.items() if pp in seeds}
 if extra<=seeds: break
 seeds|=extra
# Include the Rust driver when measuring whole experiment tree.
seeds.add(os.getppid())
print(sys.argv[1], 'pids=',sorted(seeds),'RSS_KiB=',sum(procs.get(p,(0,0,''))[1] for p in seeds))
print(subprocess.run(['lsof','-nP','-a','-p',','.join(map(str,seeds)),'-i'],capture_output=True,text=True).stdout)
for p in sorted(seeds): print(p,procs.get(p))
