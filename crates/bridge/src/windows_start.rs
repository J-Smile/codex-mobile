//! 通过已注册别名启动官方桌面，并让其代理子进程继承真实 Windows 包身份。
use crate::{BridgeError, Result};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    os::windows::ffi::OsStrExt,
    path::Path,
};
use windows_sys::Win32::{
    Foundation::CloseHandle, Storage::Packaging::Appx::GetPackageFullName, System::Threading::*,
};

pub(crate) fn package(pid: u32) -> Option<String> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut size = 0;
        let mut result = None;
        if GetPackageFullName(handle, &mut size, std::ptr::null_mut()) == 122 && size > 1 {
            let mut name = vec![0u16; size as usize];
            if GetPackageFullName(handle, &mut size, name.as_mut_ptr()) == 0 {
                result = String::from_utf16(&name[..size as usize - 1]).ok();
            }
        }
        CloseHandle(handle);
        result
    }
}

pub(crate) fn launch(command: &std::process::Command, alias: &Path) -> Result<()> {
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for (key, value) in command.get_envs() {
        environment.retain(|existing, _| !existing.eq_ignore_ascii_case(key));
        if let Some(value) = value {
            environment.insert(key.into(), value.into());
        }
    }
    let mut block = Vec::<u16>::new();
    for (key, value) in environment {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    let wide = |value: &OsStr| value.encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let executable = wide(alias.as_os_str());
    let mut arguments = wide(OsStr::new(&format!("\"{}\"", alias.display())));
    let cwd = command.get_current_dir().map(|path| wide(path.as_os_str()));
    unsafe {
        let mut size = 0;
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        let mut storage = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let attributes = storage.as_mut_ptr().cast();
        if InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let policy = 2u32;
        let result = (|| {
            if UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY as usize,
                (&policy as *const u32).cast(),
                std::mem::size_of::<u32>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            ) == 0
            {
                return Err(BridgeError::from(std::io::Error::last_os_error()));
            }
            let mut startup: STARTUPINFOEXW = std::mem::zeroed();
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            startup.lpAttributeList = attributes;
            let mut process: PROCESS_INFORMATION = std::mem::zeroed();
            if CreateProcessW(
                executable.as_ptr(),
                arguments.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT,
                block.as_ptr().cast(),
                cwd.as_ref().map_or(std::ptr::null(), |path| path.as_ptr()),
                &startup.StartupInfo,
                &mut process,
            ) == 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            Ok(())
        })();
        DeleteProcThreadAttributeList(attributes);
        result
    }
}
