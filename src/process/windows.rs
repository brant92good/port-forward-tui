use super::*;
use std::{
    mem::size_of,
    os::windows::{
        io::{AsRawHandle, AsRawSocket, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    ptr,
};
use windows_sys::Win32::{
    Foundation::{ERROR_INSUFFICIENT_BUFFER, INVALID_HANDLE_VALUE},
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
    },
    Networking::WinSock::{AF_INET, SO_EXCLUSIVEADDRUSE, SOL_SOCKET, WSAGetLastError, setsockopt},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        },
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
        Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
            CREATE_SUSPENDED, DETACHED_PROCESS, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
        },
    },
};

pub fn exclusive(socket: &socket2::Socket) -> Result<()> {
    let value = 1_i32;
    // The socket is owned and initialized by socket2; WinSock copies this value.
    let result = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&value as *const i32).cast(),
            size_of::<i32>() as i32,
        )
    };
    if result != 0 {
        return Err(std::io::Error::from_raw_os_error(unsafe { WSAGetLastError() }).into());
    }
    Ok(())
}
pub fn configure_ssh(command: &mut Command) {
    command.creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
}
pub fn readable(pipe: &std::process::ChildStderr) -> std::io::Result<usize> {
    let mut available = 0;
    let result = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            pipe.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut available,
            ptr::null_mut(),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(available as usize)
}
pub fn configure_daemon(command: &mut Command) {
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
}

pub struct Group {
    handle: Option<OwnedHandle>,
}
impl Group {
    pub fn attach(child: &mut Child) -> Result<Self> {
        // Fresh handles are immediately transferred into RAII owners. Every
        // early return closes the job; a suspended child cannot spawn descendants.
        let raw = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
        ensure!(
            !raw.is_null(),
            "Could not create the SSH process job: {}",
            std::io::Error::last_os_error()
        );
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        ensure!(
            configured != 0,
            "Could not configure SSH process ownership: {}",
            std::io::Error::last_os_error()
        );
        let assigned = unsafe { AssignProcessToJobObject(raw, child.as_raw_handle()) };
        ensure!(
            assigned != 0,
            "Could not own the SSH process: {}",
            std::io::Error::last_os_error()
        );
        resume_primary(child.id())?;
        Ok(Self {
            handle: Some(handle),
        })
    }
    pub fn stop(&mut self, child: &mut Child) {
        // Kill-on-close ends this process and any ProxyCommand descendants.
        self.handle.take();
        let _ = child.kill();
        let _ = child.wait();
    }
}
fn resume_primary(pid: u32) -> Result<()> {
    let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    ensure!(
        raw != INVALID_HANDLE_VALUE,
        "Could not inspect the suspended SSH thread"
    );
    let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut entry = THREADENTRY32 {
        dwSize: size_of::<THREADENTRY32>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Thread32First(snapshot.as_raw_handle(), &mut entry) };
    while more != 0 {
        if entry.th32OwnerProcessID == pid {
            let raw = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            ensure!(!raw.is_null(), "Could not open the suspended SSH thread");
            let thread = unsafe { OwnedHandle::from_raw_handle(raw) };
            let result = unsafe { ResumeThread(thread.as_raw_handle()) };
            ensure!(result != u32::MAX, "Could not start the owned SSH process");
            return Ok(());
        }
        more = unsafe { Thread32Next(snapshot.as_raw_handle(), &mut entry) };
    }
    anyhow::bail!("The suspended SSH process has no primary thread")
}

pub fn listeners(pids: &[u32]) -> Result<Listeners> {
    if pids.is_empty() {
        return Ok(HashSet::new());
    }
    let wanted: HashSet<_> = pids.iter().copied().collect();
    let mut size = 0_u32;
    unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0,
            AF_INET as u32,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        );
    }
    for _ in 0..4 {
        ensure!(
            (4..=16 * 1024 * 1024).contains(&size),
            "Invalid local TCP table size"
        );
        // u32 storage has sufficient alignment for the table/row structures.
        let mut buffer = vec![0_u32; (size as usize).div_ceil(4)];
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0,
                AF_INET as u32,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        ensure!(
            result == 0,
            "Could not read local TCP listener ownership (Windows error {result})"
        );
        let count = buffer[0] as usize;
        ensure!(
            size as usize <= buffer.len() * 4
                && 4 + count.saturating_mul(size_of::<MIB_TCPROW_OWNER_PID>()) <= size as usize,
            "Invalid local TCP table contents"
        );
        let rows = unsafe {
            std::slice::from_raw_parts(buffer.as_ptr().add(1).cast::<MIB_TCPROW_OWNER_PID>(), count)
        };
        return Ok(rows
            .iter()
            .filter(|row| row.dwState == 2 && wanted.contains(&row.dwOwningPid))
            .map(|row| (row.dwOwningPid, u16::from_be(row.dwLocalPort as u16)))
            .collect());
    }
    anyhow::bail!("The local TCP table kept changing; retry the ownership check")
}
