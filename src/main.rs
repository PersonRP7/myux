mod conpty;

use conpty::spawn_conpty;
use std::io::{self, Write};
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use core::ffi::c_void;

// Helper that writes all bytes to the given HANDLE and logs the result.
fn write_all(handle: HANDLE, bytes: &[u8]) {
    unsafe {
        let mut written = 0u32;
        let res = WriteFile(
            handle,
            Some(bytes),
            Some(&mut written),
            None,
        );

        eprintln!(
            "[writer] WriteFile result: {:?}, requested: {}, written: {}",
            res,
            bytes.len(),
            written,
        );
    }
}

fn main() -> windows::core::Result<()> {
    // For now, hardcode a reasonable size instead of querying console size
    let cols = 120;
    let rows = 30;

    println!("Spawning ConPTY...");
    let tab = spawn_conpty("cmd.exe", cols, rows)?; // use cmd.exe for now
    println!("ConPTY spawned, starting reader thread...");

    // Extract raw handle value as an integer (isize is Send)
    let out_raw: isize = tab.pty_out_read.0 as isize;

    let reader = std::thread::spawn(move || {
        // Reconstruct HANDLE from the raw integer inside the thread
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

    // Give the child some time to execute and print output
    std::thread::sleep(Duration::from_secs(5));

    // Drop happens here (closes handles / pseudo console) after reader finishes
    let _ = reader.join();
    println!("Done.");

    Ok(())
}