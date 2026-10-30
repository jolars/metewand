//! Authenticated loopback transport for language adapters without native bindings.

use super::*;
use std::net::{TcpListener, TcpStream};

pub(super) const TRANSPORT_ENV: &str = "METEWAND_PROTOCOL_TRANSPORT";

#[derive(Debug)]
pub(super) struct SocketEndpoint {
    state: Arc<Mutex<SocketState>>,
    canceled: Arc<AtomicBool>,
    child: Arc<Mutex<Child>>,
}

pub(super) struct SocketListener {
    listener: TcpListener,
    token: String,
    canceled: Arc<AtomicBool>,
}

struct SocketState {
    listener: Option<TcpListener>,
    stream: Option<TcpStream>,
    token: String,
}

impl SocketListener {
    pub(super) fn configure(
        command: &mut Command,
        redactor: &mut Redactor,
        canceled: Arc<AtomicBool>,
    ) -> io::Result<Option<Self>> {
        if !command
            .get_envs()
            .any(|(key, value)| key == TRANSPORT_ENV && value.is_some_and(|v| v == "socket"))
        {
            return Ok(None);
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        // Temporary names are observable by other local users, so generate
        // the authentication secret in memory.
        let mut random = [0; 32];
        getrandom::fill(&mut random).map_err(io::Error::other)?;
        let token: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
        redactor.add(token.as_bytes());
        command
            .env(
                "METEWAND_PROTOCOL_PORT",
                listener.local_addr()?.port().to_string(),
            )
            .env("METEWAND_PROTOCOL_TOKEN", &token);
        Ok(Some(Self {
            listener,
            token,
            canceled,
        }))
    }

    pub(super) fn pair(self, child: Arc<Mutex<Child>>) -> (ProtocolIo, ProtocolIo) {
        let endpoint = SocketEndpoint {
            state: Arc::new(Mutex::new(SocketState {
                listener: Some(self.listener),
                stream: None,
                token: self.token,
            })),
            canceled: self.canceled,
            child,
        };
        let other = SocketEndpoint {
            state: Arc::clone(&endpoint.state),
            canceled: Arc::clone(&endpoint.canceled),
            child: Arc::clone(&endpoint.child),
        };
        (
            ProtocolIo {
                inner: ProtocolIoKind::Socket(endpoint),
            },
            ProtocolIo {
                inner: ProtocolIoKind::Socket(other),
            },
        )
    }
}

impl SocketEndpoint {
    fn check(&self) -> io::Result<()> {
        if self.canceled.load(Ordering::Acquire) {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "worker socket cleanup canceled",
            ))
        } else {
            Ok(())
        }
    }

    fn connected(&self, state: &mut SocketState) -> io::Result<()> {
        let mut worker_exited = false;
        while state.stream.is_none() {
            self.check()?;
            let (mut stream, _) = match state
                .listener
                .as_ref()
                .expect("unconnected listener")
                .accept()
            {
                Ok(connection) => connection,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if worker_exited {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "worker exited before connecting to the protocol socket",
                        ));
                    }
                    // Retry accept after observing exit in case the worker
                    // connected between the last accept and the status check.
                    worker_exited = self
                        .child
                        .lock()
                        .map_err(|_| io::Error::other("worker child lock poisoned"))?
                        .try_wait()?
                        .is_some();
                    if !worker_exited {
                        thread::sleep(Duration::from_millis(10));
                    }
                    continue;
                }
                Err(error) => return Err(error),
            };
            configure_stream(&stream)?;
            let started = Instant::now();
            let mut authentication = Vec::new();
            let mut complete = false;
            while authentication.len() < 256 && started.elapsed() < Duration::from_secs(1) {
                self.check()?;
                let mut byte = [0];
                match stream.read(&mut byte) {
                    Ok(1) if byte[0] != b'\n' => authentication.push(byte[0]),
                    Ok(1) => {
                        complete = true;
                        break;
                    }
                    Ok(_) => break,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                        ) => {}
                    Err(_) => break,
                }
            }
            if complete && authentication == state.token.as_bytes() {
                state.stream = Some(stream);
                state.listener.take();
            }
        }
        Ok(())
    }

    fn operation<T>(
        &self,
        mut operation: impl FnMut(&mut TcpStream) -> io::Result<T>,
    ) -> io::Result<T> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| io::Error::other("worker socket lock poisoned"))?;
        self.connected(&mut state)?;
        loop {
            self.check()?;
            match operation(state.stream.as_mut().expect("authenticated socket")) {
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                    ) => {}
                result => return result,
            }
        }
    }
}

fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    // Windows accepts inherit nonblocking mode, which bypasses I/O timeouts.
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(20)))?;
    stream.set_write_timeout(Some(Duration::from_millis(20)))
}

impl Read for SocketEndpoint {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.operation(|stream| stream.read(bytes))
    }
}
impl Write for SocketEndpoint {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.operation(|stream| stream.write(bytes))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl std::fmt::Debug for SocketState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SocketState")
            .field("connected", &self.stream.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_reads_wait_for_the_timeout_on_initially_nonblocking_streams() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        // Emulate Windows inheritance on every platform to catch busy-waiting locally.
        stream.set_nonblocking(true).unwrap();
        configure_stream(&stream).unwrap();

        let started = Instant::now();
        let error = stream.read(&mut [0]).unwrap_err();
        assert!(matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ));
        assert!(started.elapsed() >= Duration::from_millis(10));
    }
}
