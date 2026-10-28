use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{broadcast, oneshot};
#[derive(Clone, Debug)]
pub struct Event {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}
type Reply = Result<Value, String>;
#[derive(Default)]
struct State {
    closed: bool,
    pending: HashMap<u64, oneshot::Sender<Reply>>,
}
impl State {
    fn close(&mut self) {
        self.closed = true;
        for (_, tx) in self.pending.drain() {
            let _ = tx.send(Err("browser closed".into()));
        }
    }
}
pub struct Browser {
    child: Child,
    writer: Mutex<std::io::PipeWriter>,
    state: Arc<Mutex<State>>,
    events: broadcast::Sender<Event>,
    next_id: AtomicU64,
}
// Reserve sources above the CDP destinations, including when the parent started
// with fd 3 or 4 vacant. F_DUPFD_CLOEXEC also prevents unintended inheritance.
fn above_stdio(fd: i32) -> std::io::Result<OwnedFd> {
    let n = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 5) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(n) })
}
pub fn launch(exe: &Path, args: &[String]) -> anyhow::Result<Browser> {
    let (request_read, request_write) = std::io::pipe()?;
    let (response_read, response_write) = std::io::pipe()?;
    let input = above_stdio(request_read.as_raw_fd())?;
    let output = above_stdio(response_write.as_raw_fd())?;
    let mut command = Command::new(exe);
    command
        .args(args)
        .arg("--remote-debugging-pipe")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(input.as_raw_fd(), 3) < 0 || libc::dup2(output.as_raw_fd(), 4) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    drop(command);
    drop(request_read);
    drop(response_write);
    let state = Arc::new(Mutex::new(State::default()));
    let reader_state = state.clone();
    let (events, _) = broadcast::channel(1024);
    let reader_events = events.clone();
    let reader = std::thread::Builder::new()
        .name("cdp-reader".into())
        .spawn(move || {
            let mut reader = response_read;
            let mut buf = Vec::new();
            let mut chunk = [0; 65536];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                for frame in split_frames(&mut buf) {
                    let Ok(message) = serde_json::from_slice::<Value>(&frame) else {
                        continue;
                    };
                    if let Some(id) = message["id"].as_u64() {
                        let tx = reader_state.lock().unwrap().pending.remove(&id);
                        if let Some(tx) = tx {
                            let reply = if let Some(error) = message.get("error") {
                                Err(format!("CDP error {}: {}", error["code"], error["message"]))
                            } else {
                                Ok(message["result"].clone())
                            };
                            let _ = tx.send(reply);
                        }
                    } else if let Some(method) = message["method"].as_str() {
                        let _ = reader_events.send(Event {
                            method: method.into(),
                            params: message["params"].clone(),
                            session_id: message["sessionId"].as_str().map(str::to_owned),
                        });
                    }
                }
            }
            reader_state.lock().unwrap().close();
        });
    if let Err(e) = reader {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.into());
    }
    Ok(Browser {
        child,
        writer: Mutex::new(request_write),
        state,
        events,
        next_id: AtomicU64::new(1),
    })
}
struct Pending {
    id: u64,
    state: Arc<Mutex<State>>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.state.lock().unwrap().pending.remove(&self.id);
    }
}
impl Browser {
    pub async fn call(
        &self,
        method: &str,
        params: Value,
        session: Option<&str>,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut request = json!({"id":id,"method":method,"params":params});
        if let Some(session) = session {
            request["sessionId"] = session.into();
        }
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(0);
        let (tx, rx) = oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            if state.closed {
                anyhow::bail!("browser closed");
            }
            state.pending.insert(id, tx);
        }
        let _pending = Pending {
            id,
            state: self.state.clone(),
        };
        if self.writer.lock().unwrap().write_all(&bytes).is_err() {
            self.state.lock().unwrap().close();
        }
        rx.await
            .map_err(|_| anyhow::anyhow!("browser closed"))?
            .map_err(anyhow::Error::msg)
    }
    pub fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
    pub fn pid(&self) -> u32 {
        self.child.id()
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self
            .writer
            .lock()
            .unwrap()
            .write_all(b"{\"id\":0,\"method\":\"Browser.close\",\"params\":{}}\0");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.state.lock().unwrap().close();
    }
}
pub(crate) fn split_frames(buf: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    let mut start = 0;
    for (i, &b) in buf.iter().enumerate() {
        if b == 0 {
            frames.push(buf[start..i].to_vec());
            start = i + 1;
        }
    }
    buf.drain(..start);
    frames
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split_frames_partial() {
        let mut b = b"{\"a\":1}\0{\"b\"".to_vec();
        assert_eq!(split_frames(&mut b), vec![b"{\"a\":1}".to_vec()]);
        assert_eq!(b, b"{\"b\"");
    }
    #[test]
    fn split_frames_many() {
        let mut b = b"a\0b\0c\0".to_vec();
        assert_eq!(
            split_frames(&mut b),
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()]
        );
        assert!(b.is_empty());
    }
    #[test]
    fn split_frames_none() {
        let mut b = b"partial".to_vec();
        assert!(split_frames(&mut b).is_empty());
        assert_eq!(b, b"partial");
    }
}
