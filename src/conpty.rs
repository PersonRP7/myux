use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::ptr::{null_mut};
use core::ffi::c_void;
use windows::Win32::System::Memory::HEAP_FLAGS;

use windows::core::{PCWSTR, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Console::{
    ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::Security::SECURITY_ATTRIBUTES;
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, PROCESS_INFORMATION, STARTUPINFOEXW,
    EXTENDED_STARTUPINFO_PRESENT, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
};
use windows::Win32::System::Memory::{HeapAlloc, HeapFree, GetProcessHeap, HEAP_ZERO_MEMORY};
use windows::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST;

pub struct TabPty {
    pub hpcon: HPCON,
    pub child_process: HANDLE,
    pub child_thread: HANDLE,
    pub pty_in_write: HANDLE,  // write keystrokes into this
    pub pty_out_read: HANDLE,  // read terminal output from this
}

impl TabPty {
    pub fn resize(&self, cols: i16, rows: i16) -> windows::core::Result<()> {
        unsafe { ResizePseudoConsole(self.hpcon, COORD { X: cols, Y: rows }) }
    }
}

impl Drop for TabPty {
    fn drop(&mut self) {
        unsafe {
            // ClosePseudoConsole closes the pseudoconsole handle
            ClosePseudoConsole(self.hpcon);

            // Close our pipe handles & process/thread handles
            let _ = CloseHandle(self.pty_in_write);
            let _ = CloseHandle(self.pty_out_read);
            let _ = CloseHandle(self.child_process);
            let _ = CloseHandle(self.child_thread);
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

// Spawns a command line attached to a new ConPTY.
// cols/rows are the initial pseudo console size.
pub fn spawn_conpty(cmdline: &str, cols: i16, rows: i16) -> Result<TabPty> {
    unsafe {
        // 1) Create pipes for ConPTY
        // ConPTY needs:
        // - an input READ handle  (ConPTY reads what you write)
        // - an output WRITE handle (ConPTY writes what you read)
        let mut pty_in_read = HANDLE::default();
        let mut pty_in_write = HANDLE::default();
        let mut pty_out_read = HANDLE::default();
        let mut pty_out_write = HANDLE::default();

        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: null_mut(),
            bInheritHandle: false.into(),
        };

        CreatePipe(&mut pty_in_read, &mut pty_in_write, Some(&sa), 0)?;
        CreatePipe(&mut pty_out_read, &mut pty_out_write, Some(&sa), 0)?;

        // 2) Create the pseudo console
        let hpcon = CreatePseudoConsole(
            COORD { X: cols, Y: rows },
            pty_in_read,    // ConPTY reads from this
            pty_out_write,  // ConPTY writes to this
            0,
        )?;

        // We can close the ends we don't use directly (ConPTY now owns them logically)
        let _ = CloseHandle(pty_in_read);
        let _ = CloseHandle(pty_out_write);

        // 3) Build STARTUPINFOEX with the PSEUDOCONSOLE attribute
        let mut si_ex: STARTUPINFOEXW = std::mem::zeroed();
        si_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;

        let mut attr_list_size: usize = 0;
        // First call: query required size, ignore the error (it will be
        // ERROR_INSUFFICIENT_BUFFER, which is expected).
        let _ = InitializeProcThreadAttributeList(
            LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            1,
            0,
            &mut attr_list_size,
        );

        // Allocate attribute list
        let heap = GetProcessHeap()?;
        let attr_list_mem = unsafe { HeapAlloc(heap, HEAP_ZERO_MEMORY, attr_list_size) };
        if attr_list_mem.is_null() {
            return Err(windows::core::Error::from_win32());
        }

        si_ex.lpAttributeList = LPPROC_THREAD_ATTRIBUTE_LIST(attr_list_mem as *mut _);

        unsafe {
            InitializeProcThreadAttributeList(
                si_ex.lpAttributeList,
                1,
                0,
                &mut attr_list_size,
            )?;
        }

        let hpcon_raw: isize = hpcon.0; // extract the inner value

        UpdateProcThreadAttribute(
            si_ex.lpAttributeList,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            // pass the raw value as the attribute "buffer"
            Some(hpcon_raw as *const c_void),
            std::mem::size_of::<isize>(),
            None,
            None,
        )?;

        // 4) Spawn child process attached to ConPTY
        // CreateProcessW requires a mutable command line buffer.
        let mut cmd = to_wide(cmdline);

        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        CreateProcessW(
            PCWSTR::null(),                 // application name (optional)
            windows::core::PWSTR(cmd.as_mut_ptr()),
            None,
            None,
            false,                          // inherit handles
            EXTENDED_STARTUPINFO_PRESENT,   // IMPORTANT
            None,
            PCWSTR::null(),
            &si_ex.StartupInfo,
            &mut pi,
        )?;

        // Cleanup attribute list
        DeleteProcThreadAttributeList(si_ex.lpAttributeList);
        HeapFree(
            heap,
            HEAP_FLAGS(0),
            Some(si_ex.lpAttributeList.0 as *const c_void),
        )?;

        Ok(TabPty {
            hpcon,
            child_process: pi.hProcess,
            child_thread: pi.hThread,
            pty_in_write,
            pty_out_read,
        })
    }
}
