//! Windows pipe I/O and atomic process-tree containment.

use super::*;
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString, c_void},
    mem::{size_of, zeroed},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, HANDLE_FLAG_INHERIT,
        INVALID_HANDLE_VALUE, LocalFree, SetHandleInformation, WAIT_FAILED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW, SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, PIPE_ACCESS_INBOUND,
        PIPE_ACCESS_OUTBOUND, ReadFile, WriteFile,
    },
    System::{
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
        JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject, TerminateJobObject,
        },
        Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE},
        Threading::{
            CREATE_UNICODE_ENVIRONMENT, CreateEventW, CreateProcessW,
            DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess,
            INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
            STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
            WaitForSingleObject,
        },
    },
};

fn owned(handle: HANDLE) -> io::Result<OwnedHandle> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: Successful Win32 creation transfers a fresh handle to this scope.
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut result: Vec<_> = value.encode_wide().collect();
    if result.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "embedded NUL in worker launch",
        ));
    }
    result.push(0);
    Ok(result)
}

impl<T: AsRawHandle> WorkerPipe<T> {
    fn operation(&self, start: impl FnOnce(*mut OVERLAPPED) -> i32) -> io::Result<usize> {
        if self.canceled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "worker pipe cleanup canceled",
            ));
        }
        // SAFETY: The event is unnamed, private, and noninheritable.
        let event = owned(unsafe { CreateEventW(null(), 1, 0, null()) })?;
        // SAFETY: Zero is the documented initial state of an OVERLAPPED operation.
        let mut operation: OVERLAPPED = unsafe { zeroed() };
        operation.hEvent = event.as_raw_handle();
        if start(&mut operation) == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                return Ok(0);
            }
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
        }
        let mut canceled = false;
        let mut wait_error = None;
        loop {
            // SAFETY: Both the event and operation remain alive until completion.
            match unsafe { WaitForSingleObject(event.as_raw_handle(), 20) } {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT if !self.canceled.load(Ordering::Acquire) => continue,
                WAIT_TIMEOUT => canceled = true,
                WAIT_FAILED => wait_error = Some(io::Error::last_os_error()),
                _ => wait_error = Some(io::Error::other("unexpected worker I/O wait status")),
            }
            // Cancellation races with completion; always collect the result before
            // the stack operation and its borrowed buffer may be released.
            unsafe { CancelIoEx(self.inner.as_raw_handle(), &operation) };
            break;
        }
        let mut transferred = 0;
        // SAFETY: This waits for completion even after cancellation and preserves
        // the operation's storage and the caller's buffer throughout that wait.
        let success = unsafe {
            GetOverlappedResult(self.inner.as_raw_handle(), &operation, &mut transferred, 1)
        };
        if let Some(error) = wait_error {
            return Err(error);
        }
        if canceled {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "worker pipe cleanup canceled",
            ));
        }
        if success == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                return Ok(0);
            }
            return Err(error);
        }
        Ok(transferred as usize)
    }
}

impl<T: AsRawHandle> Read for WorkerPipe<T> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.operation(|operation| {
            // SAFETY: The operation collects completion before this buffer expires.
            unsafe {
                ReadFile(
                    self.inner.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    buffer.len().min(u32::MAX as usize) as u32,
                    null_mut(),
                    operation,
                )
            }
        })
    }
}

impl<T: AsRawHandle> Write for WorkerPipe<T> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        self.operation(|operation| {
            // SAFETY: The operation collects completion before this buffer expires.
            unsafe {
                WriteFile(
                    self.inner.as_raw_handle(),
                    buffer.as_ptr(),
                    buffer.len().min(u32::MAX as usize) as u32,
                    null_mut(),
                    operation,
                )
            }
        })
    }

    fn flush(&mut self) -> io::Result<()> {
        // WriteFile already submits every byte. FlushFileBuffers would wait for
        // the worker to consume the pipe and defeat the session deadline.
        Ok(())
    }
}

fn pipe_pair(parent_reads: bool) -> io::Result<(File, OwnedHandle)> {
    // Names never leave this function; workers inherit already connected ends.
    let nonce = tempfile::Builder::new()
        .prefix("metewand-pipe-")
        .rand_bytes(24)
        .tempdir()?;
    let name = format!(
        r"\\.\pipe\{}-{}",
        std::process::id(),
        nonce
            .path()
            .file_name()
            .expect("temporary directory has a name")
            .to_string_lossy()
    );
    let name = wide(OsStr::new(&name))?;
    let direction = if parent_reads {
        PIPE_ACCESS_INBOUND
    } else {
        PIPE_ACCESS_OUTBOUND
    };
    struct Descriptor(*mut c_void);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    let sddl: Vec<u16> = "D:P(A;;GA;;;SY)(A;;GA;;;OW)\0".encode_utf16().collect();
    let mut descriptor = null_mut();
    // The owner-rights SID grants only the creating user and SYSTEM access.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let descriptor = Descriptor(descriptor);
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: The name is terminated and all handles stay owned during connection.
    let server = owned(unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            direction | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            65536,
            65536,
            0,
            &security,
        )
    })?;
    let access = if parent_reads {
        FILE_GENERIC_WRITE
    } else {
        FILE_GENERIC_READ
    };
    let client = owned(unsafe {
        CreateFileW(
            name.as_ptr(),
            access,
            0,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    })?;
    // The client may connect before ConnectNamedPipe starts. That condition is
    // successful connection, rather than a reason to retry another endpoint.
    let mut connection: OVERLAPPED = unsafe { zeroed() };
    if unsafe { ConnectNamedPipe(server.as_raw_handle(), &mut connection) } == 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_PIPE_CONNECTED as i32) {
            return Err(error);
        }
    }
    if unsafe {
        SetHandleInformation(
            client.as_raw_handle(),
            HANDLE_FLAG_INHERIT,
            HANDLE_FLAG_INHERIT,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((File::from(server), client))
}

#[derive(Debug)]
pub(super) struct Child {
    process: OwnedHandle,
    id: u32,
    status: Option<ExitStatus>,
    termination_requested: bool,
}

impl Child {
    pub(super) fn id(&self) -> u32 {
        self.id
    }

    pub(super) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                let status = ExitStatus::from_raw(code);
                self.status = Some(status);
                Ok(Some(status))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }

    pub(super) fn mark_terminated(&mut self) {
        self.termination_requested = true;
    }

    pub(super) fn wait(&mut self) -> io::Result<ExitStatus> {
        let timeout = if self.termination_requested {
            1000
        } else {
            INFINITE
        };
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), timeout) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "worker did not exit after forced termination",
                ));
            }
            _ => return Err(io::Error::last_os_error()),
        }
        self.try_wait()?
            .ok_or_else(|| io::Error::other("worker exit was not observable"))
    }
}

pub(super) fn abort_spawn(child: &mut Child) {
    child.mark_terminated();
    unsafe { TerminateProcess(child.process.as_raw_handle(), 1) };
    let _ = child.wait();
}

#[derive(Debug)]
pub(super) struct ProcessGroup(OwnedHandle);

impl ProcessGroup {
    fn new() -> io::Result<Self> {
        let job = owned(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(job))
    }

    pub(super) fn kill(&self) -> io::Result<()> {
        if unsafe { TerminateJobObject(self.0.as_raw_handle(), 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

struct Attributes<'a> {
    storage: Vec<usize>,
    values: std::marker::PhantomData<&'a [HANDLE]>,
}

impl<'a> Attributes<'a> {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes) };
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 2, 0, &mut bytes)
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            storage,
            values: std::marker::PhantomData,
        })
    }

    fn as_mut_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }

    fn set(&mut self, key: u32, values: &'a [HANDLE]) -> io::Result<()> {
        if unsafe {
            UpdateProcThreadAttribute(
                self.as_mut_ptr(),
                0,
                key as usize,
                values.as_ptr().cast(),
                std::mem::size_of_val(values),
                null_mut(),
                null(),
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

impl Drop for Attributes<'_> {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.as_mut_ptr()) };
    }
}

fn quote(argument: &OsStr, output: &mut Vec<u16>) -> io::Result<()> {
    let argument = wide(argument)?;
    output.push(b'"' as u16);
    let mut slashes = 0;
    for &value in &argument[..argument.len() - 1] {
        if value == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            if value == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        output.push(value);
        slashes = 0;
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

fn spawn_child(
    command: &Command,
    handles: &[HANDLE],
    group: &ProcessGroup,
    isolated: bool,
) -> io::Result<Child> {
    let executable = Path::new(command.get_program());
    if !executable.is_absolute()
        || executable
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Windows workers require a resolved absolute .exe program",
        ));
    }
    let application = wide(command.get_program())?;
    let mut arguments = Vec::new();
    quote(command.get_program(), &mut arguments)?;
    for argument in command.get_args() {
        arguments.push(b' ' as u16);
        quote(argument, &mut arguments)?;
    }
    arguments.push(0);
    let mut environment: BTreeMap<OsString, (OsString, OsString)> = BTreeMap::new();
    let key = |name: &OsStr| OsString::from(name.to_string_lossy().to_uppercase());
    if !isolated {
        for (name, value) in std::env::vars_os() {
            environment.insert(key(&name), (name, value));
        }
    }
    for (name, value) in command.get_envs() {
        if let Some(value) = value {
            environment.insert(key(name), (name.to_owned(), value.to_owned()));
        } else {
            environment.remove(&key(name));
        }
    }
    let mut block = Vec::new();
    for (_, (name, value)) in environment {
        block.extend(wide(&name)?);
        block.pop();
        block.push(b'=' as u16);
        block.extend(wide(&value)?);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    let directory = command
        .get_current_dir()
        .map(|directory| wide(directory.as_os_str()))
        .transpose()?;
    // Attribute values remain borrowed until the native list is destroyed.
    let jobs = [group.0.as_raw_handle()];
    let mut attributes = Attributes::new()?;
    attributes.set(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, handles)?;
    attributes.set(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[2];
    startup.StartupInfo.hStdOutput = handles[3];
    startup.StartupInfo.hStdError = handles[4];
    startup.lpAttributeList = attributes.as_mut_ptr();
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    if unsafe {
        CreateProcessW(
            application.as_ptr(),
            arguments.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
            block.as_ptr().cast::<c_void>(),
            directory
                .as_ref()
                .map_or(null(), |directory| directory.as_ptr()),
            &startup.StartupInfo,
            &mut process,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let thread = owned(process.hThread)?;
    drop(thread);
    Ok(Child {
        process: owned(process.hProcess)?,
        id: process.dwProcessId,
        status: None,
        termination_requested: false,
    })
}

impl WorkerProcess {
    pub(crate) fn spawn_redacted(
        command: Command,
        limits: WorkerLogLimits,
        redactor: Redactor,
    ) -> Result<Self, WorkerProcessError> {
        Self::spawn_windows(command, limits, redactor, false)
    }

    pub(crate) fn spawn_windows(
        mut command: Command,
        log_limits: WorkerLogLimits,
        mut redactor: Redactor,
        isolated: bool,
    ) -> Result<Self, WorkerProcessError> {
        let protocol_cancellation = PipeCancellation::default();
        let socket = socket::SocketListener::configure(
            &mut command,
            &mut redactor,
            Arc::clone(&protocol_cancellation.0),
        )
        .map_err(WorkerProcessError::CreateRequestPipe)?;
        let (parent_request_writer, worker_request_reader) =
            pipe_pair(false).map_err(WorkerProcessError::CreateRequestPipe)?;
        let (parent_response_reader, worker_response_writer) =
            pipe_pair(true).map_err(WorkerProcessError::CreateResponsePipe)?;
        let (stdout, worker_stdout) =
            pipe_pair(true).map_err(WorkerProcessError::CreateResponsePipe)?;
        let (stderr, worker_stderr) =
            pipe_pair(true).map_err(WorkerProcessError::CreateResponsePipe)?;
        let input = owned(unsafe {
            CreateFileW(
                wide(OsStr::new("NUL"))
                    .map_err(WorkerProcessError::Spawn)?
                    .as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        })
        .map_err(WorkerProcessError::Spawn)?;
        if unsafe {
            SetHandleInformation(
                input.as_raw_handle(),
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            )
        } == 0
        {
            return Err(WorkerProcessError::Spawn(io::Error::last_os_error()));
        }
        command
            .env_remove(PROTOCOL_READ_FD_ENV)
            .env_remove(PROTOCOL_WRITE_FD_ENV)
            .env(
                PROTOCOL_READ_HANDLE_ENV,
                (worker_request_reader.as_raw_handle() as usize).to_string(),
            )
            .env(
                PROTOCOL_WRITE_HANDLE_ENV,
                (worker_response_writer.as_raw_handle() as usize).to_string(),
            );
        let group = ProcessGroup::new().map_err(WorkerProcessError::Spawn)?;
        let mut handles = vec![
            input.as_raw_handle(),
            worker_stdout.as_raw_handle(),
            worker_stderr.as_raw_handle(),
        ];
        if socket.is_none() {
            handles.extend([
                worker_request_reader.as_raw_handle(),
                worker_response_writer.as_raw_handle(),
            ]);
        }
        let mut child =
            spawn_child(&command, &handles, &group, isolated).map_err(WorkerProcessError::Spawn)?;
        drop((
            worker_request_reader,
            worker_response_writer,
            input,
            worker_stdout,
            worker_stderr,
        ));
        let log_cancellation = PipeCancellation::default();
        let stdout_live = Arc::new(Mutex::new(LiveLog::default()));
        let stderr_live = Arc::new(Mutex::new(LiveLog::default()));
        let stdout_drain = start_log_drain(
            WorkerLogStream::Stdout,
            WorkerPipe {
                inner: stdout,
                canceled: Arc::clone(&log_cancellation.0),
            },
            log_limits.stdout_bytes,
            redactor.clone(),
            Arc::clone(&stdout_live),
        )
        .map_err(|source| {
            let _ = group.kill();
            abort_spawn(&mut child);
            WorkerProcessError::StartLogDrain {
                stream: WorkerLogStream::Stdout,
                source,
            }
        })?;
        let stderr_drain = match start_log_drain(
            WorkerLogStream::Stderr,
            WorkerPipe {
                inner: stderr,
                canceled: Arc::clone(&log_cancellation.0),
            },
            log_limits.stderr_bytes,
            redactor.clone(),
            Arc::clone(&stderr_live),
        ) {
            Ok(drain) => drain,
            Err(source) => {
                let _ = group.kill();
                abort_spawn(&mut child);
                log_cancellation.cancel();
                let _ = stdout_drain.join();
                return Err(WorkerProcessError::StartLogDrain {
                    stream: WorkerLogStream::Stderr,
                    source,
                });
            }
        };
        let child = Arc::new(Mutex::new(child));
        let (protocol_reader, protocol_writer) = if let Some(socket) = socket {
            socket.pair(Arc::clone(&child))
        } else {
            (
                WorkerPipe {
                    inner: parent_response_reader,
                    canceled: Arc::clone(&protocol_cancellation.0),
                }
                .into(),
                WorkerPipe {
                    inner: parent_request_writer,
                    canceled: Arc::clone(&protocol_cancellation.0),
                }
                .into(),
            )
        };
        Ok(Self {
            environment: None,
            redactor,
            child,
            group,
            private_working_directory: None,
            protocol_reader: Some(FrameReader::new(protocol_reader)),
            protocol_writer: Some(protocol_writer),
            protocol_cancellation,
            log_cancellation,
            stdout_drain,
            stderr_drain,
            stdout_live,
            stderr_live,
            log_limits,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_arguments_preserve_empty_values_quotes_and_trailing_backslashes() {
        for (argument, expected) in [
            ("", "\"\""),
            ("a b", "\"a b\""),
            ("a\"b", "\"a\\\"b\""),
            ("path\\", "\"path\\\\\""),
            ("a\\\"b", "\"a\\\\\\\"b\""),
        ] {
            let mut encoded = Vec::new();
            quote(OsStr::new(argument), &mut encoded).unwrap();
            assert_eq!(String::from_utf16(&encoded).unwrap(), expected);
        }
    }

    #[test]
    fn unsuccessful_termination_cannot_leave_an_unbounded_wait() {
        use windows_sys::Win32::{
            Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle},
            System::Threading::GetCurrentProcess,
        };
        let mut handle = null_mut();
        // SAFETY: Duplicating the current-process pseudo handle creates an owned
        // observation handle. This test never terminates the current process.
        let current = unsafe { GetCurrentProcess() };
        assert_ne!(
            unsafe {
                DuplicateHandle(
                    current,
                    current,
                    current,
                    &mut handle,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            },
            0
        );
        let mut child = Child {
            process: owned(handle).unwrap(),
            id: std::process::id(),
            status: None,
            termination_requested: false,
        };
        child.mark_terminated();
        let start = Instant::now();
        assert_eq!(child.wait().unwrap_err().kind(), io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn cancellation_releases_blocked_reads_and_full_pipe_writes() {
        for parent_reads in [false, true] {
            let (parent, _worker) = pipe_pair(parent_reads).unwrap();
            let canceled = Arc::new(AtomicBool::new(false));
            let mut pipe = WorkerPipe {
                inner: parent,
                canceled: Arc::clone(&canceled),
            };
            let operation = thread::spawn(move || {
                if parent_reads {
                    pipe.read(&mut [0u8; 8]).map(|_| ())
                } else {
                    pipe.write_all(&vec![b'x'; 2 * 1024 * 1024])
                }
            });
            thread::sleep(Duration::from_millis(40));
            let start = Instant::now();
            canceled.store(true, Ordering::Release);
            assert_eq!(
                operation.join().unwrap().unwrap_err().kind(),
                io::ErrorKind::TimedOut
            );
            assert!(start.elapsed() < Duration::from_secs(1));
        }
    }
}
