#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! NamiVision entry point: CLI parsing, WebView2 availability check, Tauri
//! setup and the `--selftest` headless path (spec 5.1, 7, 8.2).

mod analysis;
mod app_state;
mod audio;
mod beats;
mod commands;
mod error;
mod project;
mod selftest;

use app_state::{AppState, StartupArgs};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const USAGE: &str = "\
NamiVision 0.1.0

Usage:
  NamiVision.exe [--open <audio>] [--project <file>] [--portable]
  NamiVision.exe --selftest [--selftest-dir <dir>]

Options:
  --open <audio>        Open an audio file on startup.
  --project <file>      Open a NamiVision project on startup.
  --portable            Store settings in <exe_dir>/data.
  --selftest            Run the headless integration test and exit.
  --selftest-dir <dir>  Directory for selftest_report.json and fixtures.
  -h, --help            Show this help.";

#[derive(Default)]
struct Cli {
    open: Option<String>,
    project: Option<String>,
    portable: bool,
    selftest: bool,
    selftest_dir: Option<PathBuf>,
    help: bool,
}

impl Cli {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut cli = Self::default();
        let mut index = 0usize;

        while index < args.len() {
            let arg = &args[index];
            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (arg.as_str(), None),
            };

            match name {
                "--open" => {
                    cli.open = Some(take_value(args, &mut index, inline, name)?);
                }
                "--project" => {
                    cli.project = Some(take_value(args, &mut index, inline, name)?);
                }
                "--selftest-dir" => {
                    cli.selftest_dir =
                        Some(PathBuf::from(take_value(args, &mut index, inline, name)?));
                }
                "--portable" => cli.portable = true,
                "--selftest" => cli.selftest = true,
                "--help" | "-h" => cli.help = true,
                _ => {
                    // Unknown switches (for example a shell-injected URL) are
                    // ignored so startup can never fail on them.
                }
            }
            index += 1;
        }

        Ok(cli)
    }
}

fn take_value(
    args: &[String],
    index: &mut usize,
    inline: Option<String>,
    name: &str,
) -> Result<String, String> {
    if let Some(value) = inline {
        if value.is_empty() {
            return Err(format!("{name} requires a value"));
        }
        return Ok(value);
    }
    *index += 1;
    args.get(*index)
        .cloned()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{name} requires a value"))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match Cli::parse(&args) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    if cli.help {
        println!("{USAGE}");
        return;
    }

    if cli.selftest {
        attach_console();
        let code = selftest::run(cli.selftest_dir);
        std::process::exit(code);
    }

    let webview2 = webview2_version();
    if webview2.is_none() {
        webview2_missing_message();
        std::process::exit(1);
    }

    if let Err(error) = run_gui(cli, webview2) {
        eprintln!("NamiVision failed to start: {error}");
        std::process::exit(1);
    }
}

fn run_gui(cli: Cli, webview2: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let startup = StartupArgs {
        open_path: cli.open,
        project_path: cli.project,
    };
    let state = Arc::new(AppState::new(cli.portable, startup.clone(), webview2));

    let app = tauri::Builder::default()
        .manage(Arc::clone(&state))
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::startup_args,
            commands::pick_audio,
            commands::load_audio,
            commands::reanalyze,
            commands::set_bands,
            commands::pick_project,
            commands::load_project,
            commands::save_project,
            commands::load_settings,
            commands::save_settings,
            commands::export_beats_csv,
            commands::export_beats_csv_to,
            commands::play,
            commands::pause,
            commands::stop,
            commands::seek,
            commands::set_volume,
            commands::playback_state,
            commands::open_path,
            commands::reveal_file,
            commands::quit,
        ])
        .setup(move |app| {
            if !startup.is_empty() {
                let handle = app.handle().clone();
                let payload = startup.clone();
                // The WebView needs a moment to install its event listeners.
                // `startup_args` is also available as a pull-based fallback.
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(600));
                    use tauri::Emitter;
                    let _ = handle.emit("startup-open", payload);
                });
            }
            Ok(())
        })
        .build(tauri::generate_context!())?;

    app.run(move |app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            use tauri::Manager;
            let state = app_handle.state::<Arc<AppState>>();
            state.shutdown();
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Windows integration
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// Read the WebView2 Runtime version from the Edge Update registry keys.
#[cfg(windows)]
fn webview2_version() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};

    const CLIENT: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let candidates = [
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{CLIENT}"),
        ),
        (
            HKEY_CURRENT_USER,
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{CLIENT}"),
        ),
        (
            HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\Microsoft\EdgeUpdate\Clients\{CLIENT}"),
        ),
    ];

    for (root, subkey) in candidates {
        if let Some(version) = read_registry_string(root, &subkey, "pv") {
            if !version.trim().is_empty() {
                return Some(version);
            }
        }
    }
    None
}

#[cfg(windows)]
fn read_registry_string(
    root: windows_sys::Win32::System::Registry::HKEY,
    subkey: &str,
    value_name: &str,
) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, KEY_READ,
    };

    let subkey = wide(subkey);
    let value_name = wide(value_name);
    let mut key: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();

    // SAFETY: `subkey` is a valid null-terminated UTF-16 string and `key` is a
    // valid out-pointer.  The handle is closed on every success path below.
    let status = unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_READ, &mut key) };
    if status != 0 {
        return None;
    }

    let mut buffer = [0u16; 512];
    let mut size = (buffer.len() * 2) as u32;
    let mut kind = 0u32;
    // SAFETY: the buffer and size describe the same allocation; `kind` is a
    // valid out-pointer.
    let status = unsafe {
        RegQueryValueExW(
            key,
            value_name.as_ptr(),
            std::ptr::null(),
            &mut kind,
            buffer.as_mut_ptr() as *mut u8,
            &mut size,
        )
    };
    // SAFETY: `key` was opened successfully above.
    unsafe {
        RegCloseKey(key);
    }

    if status != 0 {
        return None;
    }

    let chars = (size as usize / 2).min(buffer.len());
    let end = buffer[..chars]
        .iter()
        .position(|c| *c == 0)
        .unwrap_or(chars);
    String::from_utf16(&buffer[..end]).ok()
}

#[cfg(not(windows))]
fn webview2_version() -> Option<String> {
    Some("not-checked".to_string())
}

#[cfg(windows)]
fn webview2_missing_message() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let text = wide(
        "NamiVision requires the Microsoft Edge WebView2 Runtime.\n\n\
         Install it from https://go.microsoft.com/fwlink/p/?LinkId=2124703\n\
         and start NamiVision again.",
    );
    let caption = wide("NamiVision");
    // SAFETY: both strings are valid null-terminated UTF-16 buffers.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn webview2_missing_message() {}

/// In a release build (`windows_subsystem = "windows"`) `--selftest` has no
/// console; attach to the parent console and rebind stdout/stderr so the
/// report is visible.  Failure is harmless because the JSON file is still
/// written (spec 5.1).
///
/// Handles that the launcher already provided (for example a redirected pipe
/// when the report is captured by a script or CI) are kept as-is so the text
/// still reaches the caller instead of the console screen buffer.
#[cfg(windows)]
fn attach_console() {
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_OUTPUT_HANDLE,
    };

    fn usable(handle: *mut core::ffi::c_void) -> bool {
        !handle.is_null() && handle != INVALID_HANDLE_VALUE
    }

    // SAFETY: documented calls that only read the inherited standard handles.
    let (stdout_ok, stderr_ok) = unsafe {
        (
            usable(GetStdHandle(STD_OUTPUT_HANDLE)),
            usable(GetStdHandle(STD_ERROR_HANDLE)),
        )
    };
    if stdout_ok && stderr_ok {
        return;
    }

    // SAFETY: documented call with the pseudo-process id.
    let attached = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    if attached == 0 {
        return;
    }

    if let Ok(console) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")
    {
        use std::os::windows::io::AsRawHandle;
        let handle = console.as_raw_handle();
        // SAFETY: the handle remains valid because the File is leaked below.
        unsafe {
            if !stdout_ok {
                SetStdHandle(STD_OUTPUT_HANDLE, handle);
            }
            if !stderr_ok {
                SetStdHandle(STD_ERROR_HANDLE, handle);
            }
        }
        std::mem::forget(console);
    }
}

#[cfg(not(windows))]
fn attach_console() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_all_documented_switches() {
        let args: Vec<String> = [
            "--open",
            "C:\\music\\a.wav",
            "--project",
            "C:\\work\\b.json",
            "--portable",
            "--selftest",
            "--selftest-dir",
            "C:\\tmp\\selftest",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let cli = Cli::parse(&args).expect("parse");
        assert_eq!(cli.open.as_deref(), Some("C:\\music\\a.wav"));
        assert_eq!(cli.project.as_deref(), Some("C:\\work\\b.json"));
        assert!(cli.portable);
        assert!(cli.selftest);
        assert_eq!(
            cli.selftest_dir.as_deref(),
            Some(std::path::Path::new("C:\\tmp\\selftest"))
        );
    }

    #[test]
    fn cli_supports_inline_values_and_ignores_unknown_args() {
        let args: Vec<String> = ["--open=a.mp3", "--unknown", "value", "--help"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let cli = Cli::parse(&args).expect("parse");
        assert_eq!(cli.open.as_deref(), Some("a.mp3"));
        assert!(cli.help);
        assert!(!cli.portable);
    }

    #[test]
    fn cli_missing_value_is_an_error() {
        let args: Vec<String> = ["--open"].iter().map(|s| s.to_string()).collect();
        assert!(Cli::parse(&args).is_err());
    }
}
