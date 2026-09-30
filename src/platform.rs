//! Thin OS-specific helpers (thread priority, console, timer resolution, autostart,
//! single-instance handling, virtual microphone driver).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A `Command` that never flashes a console window (the app is a GUI-subsystem exe).
fn quiet_command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(windows)]
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Attach to the parent console so `println!` works from a GUI-subsystem exe.
#[cfg(windows)]
pub fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // SAFETY: plain Win32 call with a documented constant argument.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
#[cfg(not(windows))]
pub fn attach_parent_console() {}

/// Ask the scheduler for 1 ms timer granularity so short sleeps are accurate.
#[cfg(windows)]
pub fn set_timer_resolution_1ms() {
    use windows_sys::Win32::Media::timeBeginPeriod;
    // SAFETY: plain Win32 call.
    unsafe {
        timeBeginPeriod(1);
    }
}
#[cfg(not(windows))]
pub fn set_timer_resolution_1ms() {}

/// Promote the calling thread to real-time audio priority (MMCSS "Pro Audio").
/// Returns true when MMCSS accepted the request.
#[cfg(windows)]
pub fn promote_audio_thread() -> bool {
    use windows_sys::Win32::System::Threading::{
        AvSetMmThreadCharacteristicsW, GetCurrentThread, SetThreadPriority,
        THREAD_PRIORITY_TIME_CRITICAL,
    };
    let task = wide("Pro Audio");
    let mut index: u32 = 0;
    // SAFETY: `task` is a valid NUL-terminated UTF-16 string that outlives the call.
    let handle = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut index) };
    if handle.is_null() {
        // SAFETY: plain Win32 calls on the current thread handle.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
        }
        false
    } else {
        true
    }
}
#[cfg(not(windows))]
pub fn promote_audio_thread() -> bool {
    false
}

/// Guard that keeps a named mutex alive so only one instance of the app runs.
pub struct SingleInstance {
    #[cfg(windows)]
    _handle: *mut core::ffi::c_void,
}

// SAFETY: the handle is only used to keep the mutex alive; it is never dereferenced.
unsafe impl Send for SingleInstance {}

impl SingleInstance {
    /// Returns `None` if another instance already owns the mutex.
    #[cfg(windows)]
    pub fn acquire(name: &str) -> Option<Self> {
        use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name = wide(&format!("Local\\{name}"));
        // SAFETY: `name` is a valid NUL-terminated UTF-16 string.
        let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if handle.is_null() {
            return Some(Self { _handle: handle });
        }
        // SAFETY: plain Win32 call.
        let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        if already { None } else { Some(Self { _handle: handle }) }
    }
    #[cfg(not(windows))]
    pub fn acquire(_name: &str) -> Option<Self> {
        Some(Self {})
    }
}

#[cfg(windows)]
const SHOW_EVENT: &str = "Local\\ClearMic.ShowWindow";

/// Ask an already running instance to bring its window to the front.
/// Returns true if such an instance was found and signalled.
#[cfg(windows)]
pub fn signal_show_window() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
    let name = wide(SHOW_EVENT);
    // SAFETY: `name` is a valid NUL-terminated UTF-16 string; the handle is closed below.
    unsafe {
        let handle = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if handle.is_null() {
            return false;
        }
        let ok = SetEvent(handle) != 0;
        CloseHandle(handle);
        ok
    }
}
#[cfg(not(windows))]
pub fn signal_show_window() -> bool {
    false
}

/// Run `callback` every time another launch of the app calls [`signal_show_window`].
#[cfg(windows)]
pub fn on_show_window_signal<F: Fn() + Send + 'static>(callback: F) {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
    let name = wide(SHOW_EVENT);
    // SAFETY: `name` is a valid NUL-terminated UTF-16 string. Auto-reset, initially unset.
    let handle = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
    if handle.is_null() {
        return;
    }
    let handle = handle as usize; // raw handles are plain integers; make the closure Send
    let _ = std::thread::Builder::new()
        .name("clearmic-show-signal".into())
        .spawn(move || {
            loop {
                // SAFETY: the event handle stays open for the lifetime of the process.
                let r = unsafe { WaitForSingleObject(handle as *mut core::ffi::c_void, INFINITE) };
                if r != WAIT_OBJECT_0 {
                    break;
                }
                callback();
            }
        });
}
#[cfg(not(windows))]
pub fn on_show_window_signal<F: Fn() + Send + 'static>(_callback: F) {}

// ------------------------------------------------------------------ autostart

const RUN_KEY_USER: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_KEY_MACHINE: &str = r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run";

fn reg_value_exists(key: &str, value: &str) -> bool {
    quiet_command("reg")
        .args(["query", key, "/v", value])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Register or remove the app in the per-user startup list.
pub fn set_autostart(app_name: &str, enable: bool) -> anyhow::Result<()> {
    if !cfg!(windows) {
        anyhow::bail!("autostart is only supported on Windows");
    }
    if enable {
        let exe = std::env::current_exe()?;
        let output = quiet_command("reg")
            .args([
                "add",
                RUN_KEY_USER,
                "/v",
                app_name,
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\" --minimized", exe.display()),
                "/f",
            ])
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "reg.exe failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    } else {
        let _ = quiet_command("reg")
            .args(["delete", RUN_KEY_USER, "/v", app_name, "/f"])
            .output()?;
        if reg_value_exists(RUN_KEY_MACHINE, app_name) {
            anyhow::bail!(
                "autostart was enabled for all users by the installer; an administrator has to remove it"
            );
        }
    }
    Ok(())
}

/// Whether the app is registered to start with Windows (per user or machine-wide).
pub fn autostart_enabled(app_name: &str) -> bool {
    cfg!(windows)
        && (reg_value_exists(RUN_KEY_USER, app_name) || reg_value_exists(RUN_KEY_MACHINE, app_name))
}

/// Open a URL, settings page or folder with the default handler (best effort).
pub fn open_url(target: &str) {
    #[cfg(windows)]
    {
        let _ = quiet_command("explorer.exe").arg(target).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("xdg-open").arg(target).spawn();
    }
}

// ------------------------------------------------- virtual microphone driver

const VBCABLE_SERVICE_KEY: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\VBAudioVACMME";

/// Path of the VB-CABLE setup program shipped next to the executable, if present.
pub fn bundled_driver_setup() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let setup = exe.parent()?.join("vbcable").join("VBCABLE_Setup_x64.exe");
    setup.exists().then_some(setup)
}

/// True when the VB-CABLE driver is registered with Windows. Its audio endpoints can
/// still be missing until the next reboot.
pub fn virtual_mic_driver_present() -> bool {
    cfg!(windows)
        && quiet_command("reg")
            .args(["query", VBCABLE_SERVICE_KEY])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
}

/// Run the bundled VB-CABLE setup silently with administrator rights (Windows shows one
/// consent prompt). Blocks until the setup program exits.
pub fn install_virtual_mic_driver(setup: &Path) -> anyhow::Result<()> {
    if !cfg!(windows) {
        anyhow::bail!("the virtual microphone driver is only available on Windows");
    }
    let quote = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let dir = setup.parent().unwrap_or(Path::new("."));
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         $p = Start-Process -FilePath '{}' -ArgumentList '-i','-h' -WorkingDirectory '{}' \
         -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
        quote(setup),
        quote(dir)
    );
    let output = quiet_command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()?;
    if virtual_mic_driver_present() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("the installation was cancelled")
        .trim()
        .to_string();
    anyhow::bail!("{reason}")
}

/// Show a simple message box without pulling in extra UI dependencies (best effort).
pub fn message_box(title: &str, text: &str) {
    if !cfg!(windows) {
        eprintln!("{title}: {text}");
        return;
    }
    let esc = |v: &str| v.replace('\'', "''");
    let script = format!(
        "Add-Type -AssemblyName PresentationFramework; [void][System.Windows.MessageBox]::Show('{}', '{}')",
        esc(text),
        esc(title)
    );
    let _ = quiet_command("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .spawn();
}
