use crate::{BridgeError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started: String,
    pub executable: PathBuf,
}

impl ProcessIdentity {
    pub fn current() -> Result<Self> {
        Self::read(std::process::id())
            .ok_or_else(|| BridgeError::unavailable("无法确认当前进程身份。"))
    }
    pub fn alive(&self) -> bool {
        Self::read(self.pid).as_ref() == Some(self)
    }
    pub fn read(pid: u32) -> Option<Self> {
        if pid == 0 {
            return None;
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{
                Foundation::{CloseHandle, FILETIME},
                System::Threading::{
                    GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
                    PROCESS_QUERY_LIMITED_INFORMATION,
                },
            };
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return None;
            }
            let result = (|| {
                let mut code = 0;
                if GetExitCodeProcess(handle, &mut code) == 0 || code != 259 {
                    return None;
                }
                let mut created: FILETIME = std::mem::zeroed();
                let mut exited: FILETIME = std::mem::zeroed();
                let mut kernel: FILETIME = std::mem::zeroed();
                let mut user: FILETIME = std::mem::zeroed();
                if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
                    return None;
                }
                let mut path = vec![0u16; 32768];
                let mut length = path.len() as u32;
                if QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) == 0 {
                    return None;
                }
                Some(Self {
                    pid,
                    started: (((created.dwHighDateTime as u64) << 32)
                        | created.dwLowDateTime as u64)
                        .to_string(),
                    executable: PathBuf::from(String::from_utf16_lossy(&path[..length as usize])),
                })
            })();
            CloseHandle(handle);
            result
        }
        #[cfg(target_os = "linux")]
        {
            let root = PathBuf::from(format!("/proc/{pid}"));
            let stat = std::fs::read_to_string(root.join("stat")).ok()?;
            let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            if fields.first() == Some(&"Z") {
                return None;
            }
            Some(Self {
                pid,
                started: fields.get(19)?.to_string(),
                executable: std::fs::read_link(root.join("exe")).ok()?,
            })
        }
        #[cfg(target_os = "macos")]
        {
            let output = std::process::Command::new("ps")
                .args(["-p", &pid.to_string(), "-o", "lstart=,comm="])
                .output()
                .ok()?;
            let record = String::from_utf8(output.stdout).ok()?;
            let record = record.trim();
            if record.len() <= 24 {
                return None;
            }
            Some(Self {
                pid,
                started: record[..24].to_string(),
                executable: PathBuf::from(record[24..].trim()),
            })
        }
    }
}

pub fn parent_pid(pid: u32) -> Option<u32> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
            System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
        };
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = Process32FirstW(snapshot, &mut entry);
        let mut result = None;
        while found != 0 {
            if entry.th32ProcessID == pid {
                result = Some(entry.th32ParentProcessID);
                break;
            }
            found = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        result
    }
    #[cfg(unix)]
    {
        let output = std::process::Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "ppid="])
            .output()
            .ok()?;
        String::from_utf8(output.stdout).ok()?.trim().parse().ok()
    }
}
