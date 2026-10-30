use super::*;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

pub(crate) enum TreeSource {
    #[cfg(target_os = "linux")]
    Linux(u32),
    #[cfg(windows)]
    Windows(std::os::windows::io::OwnedHandle),
    Unavailable,
}

impl TreeSource {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            #[cfg(target_os = "linux")]
            Self::Linux(_) => "linux_proc_process_group_sampled",
            #[cfg(windows)]
            Self::Windows(_) => "windows_job_object",
            Self::Unavailable => "unavailable",
        }
    }

    fn sample(&self) -> Option<BTreeMap<(u32, u64), u64>> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Linux(group) => {
                let ticks = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).ok()?;
                let mut result = BTreeMap::new();
                for entry in std::fs::read_dir("/proc").ok()?.flatten() {
                    let Some(pid) = entry
                        .file_name()
                        .to_str()
                        .and_then(|s| s.parse::<u32>().ok())
                    else {
                        continue;
                    };
                    let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
                        continue;
                    };
                    let Some((_, tail)) = stat.rsplit_once(')') else {
                        continue;
                    };
                    let fields: Vec<_> = tail.split_whitespace().collect();
                    if fields.get(2).and_then(|s| s.parse::<u32>().ok()) != Some(*group) {
                        continue;
                    }
                    if let Some((start, cpu)) = fields
                        .get(19)
                        .and_then(|s| s.parse::<u64>().ok())
                        .zip(super::parse_cpu_time(&stat, ticks))
                    {
                        result.insert((pid, start), cpu);
                    }
                }
                (!result.is_empty()).then_some(result)
            }
            #[cfg(windows)]
            Self::Windows(handle) => {
                use std::os::windows::io::AsRawHandle;
                use windows_sys::Win32::System::JobObjects::{
                    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JobObjectBasicAccountingInformation,
                    QueryInformationJobObject,
                };
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION =
                    unsafe { std::mem::zeroed() };
                // The owned job handle remains live throughout the query.
                let ok = unsafe {
                    QueryInformationJobObject(
                        handle.as_raw_handle(),
                        JobObjectBasicAccountingInformation,
                        (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                        std::mem::size_of_val(&info) as u32,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return None;
                }
                let total = u64::try_from(info.TotalUserTime.checked_add(info.TotalKernelTime)?)
                    .ok()?
                    .checked_mul(100)?;
                Some(BTreeMap::from([((0, 0), total)]))
            }
            Self::Unavailable => None,
        }
    }
}

#[derive(Default)]
struct Accumulator {
    active: bool,
    started: bool,
    previous: BTreeMap<(u32, u64), u64>,
    total: u64,
    observed: bool,
    lost: bool,
}
impl Accumulator {
    fn sample(&mut self, sample: Option<BTreeMap<(u32, u64), u64>>) {
        let Some(sample) = sample else {
            self.lost = true;
            return;
        };
        self.observed = true;
        if self.active {
            for (id, value) in &sample {
                let baseline = self.previous.get(id).copied().unwrap_or(0);
                if let Some(delta) = value.checked_sub(baseline) {
                    self.total = self.total.saturating_add(delta);
                } else {
                    self.lost = true;
                }
            }
        }
        // Retain exited identities so a transient read failure cannot count their CPU twice.
        self.previous.extend(sample);
    }
}

pub(crate) struct TreeMonitor {
    source: Arc<TreeSource>,
    state: Arc<Mutex<Accumulator>>,
    stop: mpsc::Sender<()>,
    thread: Option<thread::JoinHandle<()>>,
}
impl TreeMonitor {
    pub(crate) fn new(source: TreeSource) -> std::io::Result<Self> {
        let source = Arc::new(source);
        let mut state = Accumulator::default();
        state.sample(source.sample());
        let state = Arc::new(Mutex::new(state));
        let (stop, receiver) = mpsc::channel();
        let background_source = source.clone();
        let background_state = state.clone();
        let thread = thread::Builder::new()
            .name("metewand-tree-accounting".into())
            .spawn(move || {
                while matches!(
                    receiver.recv_timeout(Duration::from_millis(10)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    let mut state = background_state.lock().expect("accounting lock");
                    if state.active {
                        state.sample(background_source.sample());
                    }
                }
            })?;
        Ok(Self {
            source,
            state,
            stop,
            thread: Some(thread),
        })
    }
    pub(crate) fn name(&self) -> &'static str {
        self.source.name()
    }
    pub(crate) fn begin_from_launch(&self) {
        let mut state = self.state.lock().expect("accounting lock");
        state.previous.clear();
        state.active = true;
        state.started = true;
        state.sample(self.source.sample());
    }
    pub(crate) fn resume(&self) {
        let mut state = self.state.lock().expect("accounting lock");
        state.sample(self.source.sample());
        state.active = true;
        state.started = true;
    }
    pub(crate) fn pause(&self) -> Measurement {
        let mut state = self.state.lock().expect("accounting lock");
        if !state.started {
            return Measurement::unavailable(MissingReason::NotStarted);
        }
        state.sample(self.source.sample());
        state.active = false;
        if self.source.name() == "unavailable" {
            return Measurement::unavailable(MissingReason::Unsupported);
        }
        if !state.observed {
            return Measurement::unavailable(MissingReason::AccountingUnavailable);
        }
        if state.total > crate::core::canonical::MAX_SAFE_INTEGER {
            return Measurement::unavailable(MissingReason::OutOfRange);
        }
        if cfg!(windows) && !state.lost {
            Measurement::Complete { value: state.total }
        } else {
            Measurement::Partial {
                value: state.total,
                reason: if cfg!(windows) {
                    PartialReason::Interrupted
                } else {
                    PartialReason::SampledProcessTree
                },
            }
        }
    }
}
impl Drop for TreeMonitor {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paused_children_and_pid_reuse_do_not_pollute_active_cpu() {
        let mut state = Accumulator::default();
        state.sample(Some(BTreeMap::from([((1, 10), 100)])));
        state.active = true;
        state.sample(Some(BTreeMap::from([((1, 10), 120), ((2, 20), 30)])));
        assert_eq!(state.total, 50);
        state.active = false;
        state.sample(Some(BTreeMap::from([((1, 10), 1000), ((2, 20), 900)])));
        state.active = true;
        state.sample(Some(BTreeMap::from([((1, 10), 1010), ((2, 21), 7)])));
        assert_eq!(state.total, 67);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_accounting_includes_cpu_from_a_live_child() {
        use std::{
            io::{BufRead, BufReader, Write},
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let code = r#"
import subprocess, sys
sys.stdout.write('ready\n'); sys.stdout.flush()
sys.stdin.readline()
child = subprocess.Popen([sys.executable, '-c', "import time,sys; start=time.process_time(); exec('while time.process_time()-start < 0.05: pass'); print('done',flush=True); sys.stdin.readline()"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
child.stdout.readline()
print(child.pid,flush=True)
sys.stdin.readline()
child.stdin.write(b'end\n'); child.stdin.flush(); child.wait()
"#;
        let mut worker = Command::new("python3")
            .args(["-c", code])
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = BufReader::new(worker.stdout.take().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        let monitor = TreeMonitor::new(TreeSource::Linux(worker.id())).unwrap();
        monitor.resume();
        worker.stdin.as_mut().unwrap().write_all(b"go\n").unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let child: u32 = line.trim().parse().unwrap();
        let cpu = monitor.pause();
        worker.stdin.as_mut().unwrap().write_all(b"end\n").unwrap();
        assert!(worker.wait().unwrap().success());
        assert!(
            monitor
                .state
                .lock()
                .unwrap()
                .previous
                .keys()
                .any(|(pid, _)| *pid == child)
        );
        assert!(
            matches!(cpu, Measurement::Partial { value,reason:PartialReason::SampledProcessTree } if value >=40_000_000),
            "{cpu:?}"
        );
    }
}
