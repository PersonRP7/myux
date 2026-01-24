mod conpty;

use conpty::spawn_conpty;
use std::io::{self, Write};
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::ReadFile;
use core::ffi::c_void;

// keep your write_all helper
fn write_all(handle: HANDLE, bytes: &[u8]) {
    unsafe {
        let mut written = 0u32;
        let _ = windows::Win32::Storage::FileSystem::WriteFile(
            handle,
            Some(bytes),
            Some(&mut written),
            None,
        );
    }
}

fn main() -> windows::core::Result<()> {
    // For now, hardcode a reasonable size
    let cols = 120;
    let rows = 30;

    println!("Spawning ConPTY...");
    let tab = spawn_conpty("cmd.exe", cols, rows)?; // use cmd.exe for now
    println!("ConPTY spawned, starting reader thread...");

    // ---- Reader thread exactly like before, but add debug prints ----
    let out_raw = tab.pty_out_read.0 as usize;

    let reader = std::thread::spawn(move || {
        let out_handle = HANDLE(out_raw as *mut c_void);
        let mut buf = [0u8; 8192];

        eprintln!("[reader] thread started");

        loop {
            unsafe {
                let mut read = 0u32;

                let res = ReadFile(
                    out_handle,
                    Some(&mut buf),
                    Some(&mut read),
                    None,
                );

                if let Err(err) = res {
                    eprintln!("[reader] ReadFile error: {err:?}");
                    break;
                }

                if read == 0 {
                    eprintln!("[reader] ReadFile returned 0 bytes, exiting");
                    break;
                }

                eprintln!("[reader] got {read} bytes");
                let _ = io::stdout().write_all(&buf[..read as usize]);
                let _ = io::stdout().flush();
            }
        }

        eprintln!("[reader] exiting");
    });

    // ---- Send a test command without any raw-mode / event stuff ----
    println!("Writing \"dir\" to the pseudo console...");
    write_all(tab.pty_in_write, b"dir\r\n");

    // Give it a bit of time to run and print
    std::thread::sleep(Duration::from_secs(3));

    // Drop happens here (closes handles / pseudo console)
    let _ = reader.join();
    println!("Done.");

    Ok(())
}