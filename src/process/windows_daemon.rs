//! Windows daemon creation with an explicit inherited-handle list.
//!
//! A caller can inherit extra pipe handles through several shell wrappers.
//! Redirecting standard streams alone does not exclude those handles from
//! stable Rust's Command spawn. Keep that boundary here, at the long-lived
//! controller, without changing the caller's handles or environment.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
    mem::{size_of, size_of_val},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::ExitStatusExt,
    },
    path::Path,
    process::ExitStatus,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    },
    System::Threading::{
        CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW,
        DETACHED_PROCESS, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
        GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
        STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute,
        WaitForSingleObject,
    },
};

/// Closing this process handle does not terminate a background controller.
pub struct DaemonChild(OwnedHandle);

impl DaemonChild {
    /// Terminate only this owned process. Callers must bound any subsequent wait.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.try_wait()?.is_some() {
            return Ok(());
        }
        if unsafe { TerminateProcess(self.0.as_raw_handle(), 1) } == 0 {
            let error = io::Error::last_os_error();
            if self.try_wait()?.is_none() {
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // Waiting on the owned process handle also distinguishes an actual
        // exit code of STILL_ACTIVE (259) from a process that is still running.
        match unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0;
                if unsafe { GetExitCodeProcess(self.0.as_raw_handle(), &mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
}

fn wide(value: &OsStr) -> Result<Vec<u16>> {
    let mut units: Vec<u16> = value.encode_wide().collect();
    ensure!(
        !units.contains(&0),
        "A process argument contains a NUL character."
    );
    units.push(0);
    Ok(units)
}

/// Quote one ordinary Windows argv argument, including trailing backslashes.
fn argument(value: &OsStr, output: &mut Vec<u16>) -> Result<()> {
    let units = wide(value)?;
    output.push(b'"' as u16);
    let mut slashes = 0;
    for &unit in &units[..units.len() - 1] {
        if unit == b'\\' as u16 {
            slashes += 1;
            continue;
        }
        output.extend(std::iter::repeat_n(
            b'\\' as u16,
            if unit == b'"' as u16 {
                slashes * 2 + 1
            } else {
                slashes
            },
        ));
        slashes = 0;
        output.push(unit);
    }
    output.extend(std::iter::repeat_n(b'\\' as u16, slashes * 2));
    output.push(b'"' as u16);
    Ok(())
}

fn inheritable(file: &File) -> io::Result<OwnedHandle> {
    let mut duplicate = ptr::null_mut();
    // Duplicate only our two newly opened files, never the caller's handles.
    let process = unsafe { GetCurrentProcess() };
    if unsafe {
        DuplicateHandle(
            process,
            file.as_raw_handle(),
            process,
            &mut duplicate,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

struct Attributes {
    // Pointer-sized storage provides alignment and remains allocated until
    // DeleteProcThreadAttributeList has run. Vec never changes after init.
    storage: Vec<usize>,
}
impl Attributes {
    fn new() -> Result<Self> {
        let mut bytes = 0;
        let result =
            unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut bytes) };
        let error = io::Error::last_os_error();
        ensure!(
            result == 0 && error.raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32),
            "Could not size process attributes: {error}"
        );
        ensure!(
            bytes > 0 && bytes <= 64 * 1024,
            "Unexpected process attribute size."
        );
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        if unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut bytes)
        } == 0
        {
            return Err(io::Error::last_os_error())
                .context("Could not initialize process attributes");
        }
        // Construct the RAII owner only after successful initialization.
        Ok(Self { storage })
    }
    fn pointer(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.pointer()) };
    }
}

pub(super) fn spawn(executable: &Path, directory: &Path, log: File) -> Result<DaemonChild> {
    // BREAKAWAY remains mandatory: a restricted host job must fail explicitly.
    create(
        executable,
        &[
            OsStr::new("--serve"),
            OsStr::new("--data-dir"),
            directory.as_os_str(),
        ],
        log,
        DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB,
    )
}

pub(super) fn hidden(executable: &Path, args: &[OsString]) -> Result<DaemonChild> {
    create(
        executable,
        &args.iter().map(OsString::as_os_str).collect::<Vec<_>>(),
        File::options().write(true).open("NUL")?,
        CREATE_NO_WINDOW,
    )
}

fn create(executable: &Path, args: &[&OsStr], log: File, flags: u32) -> Result<DaemonChild> {
    ensure!(
        executable.is_absolute(),
        "The executable must be an absolute path."
    );
    let application = wide(executable.as_os_str())?;
    let mut command = Vec::new();
    for value in std::iter::once(executable.as_os_str()).chain(args.iter().copied()) {
        if !command.is_empty() {
            command.push(b' ' as u16);
        }
        argument(value, &mut command)?;
    }
    command.push(0);
    ensure!(command.len() <= 32_767, "The command line is too long.");

    let input = inheritable(&File::open("NUL")?)?;
    let output = inheritable(&log)?;
    let handles: [HANDLE; 2] = [input.as_raw_handle(), output.as_raw_handle()];
    let mut attributes = Attributes::new()?;
    ensure!(
        unsafe {
            UpdateProcThreadAttribute(
                attributes.pointer(),
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(&handles),
                ptr::null_mut(),
                ptr::null(),
            )
        } != 0,
        "Could not restrict child handle inheritance: {}",
        io::Error::last_os_error()
    );
    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = handles[0];
    startup.StartupInfo.hStdOutput = handles[1];
    startup.StartupInfo.hStdError = handles[1];
    startup.lpAttributeList = attributes.pointer();
    let mut child = PROCESS_INFORMATION::default();
    // Both attribute storage and the handle array stay alive for this call.
    // NULL environment and cwd preserve the developer's current environment.
    ensure!(
        unsafe {
            CreateProcessW(
                application.as_ptr(),
                command.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                1,
                EXTENDED_STARTUPINFO_PRESENT | flags,
                ptr::null(),
                ptr::null(),
                &startup.StartupInfo,
                &mut child,
            )
        } != 0,
        "Could not create the process: {}",
        io::Error::last_os_error()
    );
    let process = unsafe { OwnedHandle::from_raw_handle(child.hProcess) };
    let _thread = unsafe { OwnedHandle::from_raw_handle(child.hThread) };
    Ok(DaemonChild(process))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_preserve_quotes_backslashes_empty_and_unicode() {
        for (input, expected) in [
            ("", "\"\""),
            ("space path", "\"space path\""),
            ("臺中 😀's", "\"臺中 😀's\""),
            ("C:\\trailing\\", "\"C:\\trailing\\\\\""),
            ("a\\\"b", "\"a\\\\\\\"b\""),
        ] {
            let mut output = Vec::new();
            argument(OsStr::new(input), &mut output).unwrap();
            assert_eq!(String::from_utf16(&output).unwrap(), expected);
        }
        assert!(argument(OsStr::new("prefix\0suffix"), &mut Vec::new()).is_err());
    }
}
