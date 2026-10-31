//! The Windows side: the game's processes, a hidden message window (shutdown hold, one Satchel
//! per PC), start with Windows, the desktop shortcut, notifications, network paths, folders.

use satchel_core::log;
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use tauri_winrt_notification::{Duration as ToastDuration, Toast};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, GetLastError, HWND, LPARAM, LRESULT, RECT,
    WPARAM,
};
use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW};
use windows::Win32::System::Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy};
use windows::Win32::Storage::FileSystem::{
    GetFileVersionInfoSizeW, GetFileVersionInfoW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, VS_FIXEDFILEINFO,
    VerQueryValueW,
};
use windows::Win32::System::Threading::{
    CreateMutexW, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    QueryFullProcessImageNameW, WaitForSingleObject,
};
use windows::Win32::UI::Shell::{FOLDERID_Desktop, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW, GetWindowThreadProcessId,
    HWND_BROADCAST, MSG, PostMessageW, RegisterClassW,
    RegisterWindowMessageW, SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
    TranslateMessage, WINDOW_EX_STYLE, WM_APP, WM_ENDSESSION, WM_KEYDOWN, WM_KEYUP, WM_QUERYENDSESSION, WNDCLASSW,
    WS_OVERLAPPED,
};
use windows::core::{HSTRING, IUnknown, Interface, PCWSTR, PWSTR, w};

pub const GAME_EXE: &str = "AzerothSurvivors.exe";
const APP_ID: &str = "SurvivorsSatchel";

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

// One Satchel per PC ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
pub enum Request {
    Show,
    Play,
}

fn show_message() -> u32 {
    unsafe { RegisterWindowMessageW(w!("SurvivorsSatchel.Show")) }
}

fn play_message() -> u32 {
    unsafe { RegisterWindowMessageW(w!("SurvivorsSatchel.Play")) }
}

/// `true` for the first Satchel of this Windows user. A second start hands its request (show the
/// window, or play) to the running one and gets `false`. A Satchel started by an update looks
/// again for up to `wait` while the one it replaces is still there: a slow last send held the old
/// one past the wait for its exit, and the new one gave way to it, which left no Satchel running.
pub fn first_instance(request: Request, wait: Duration) -> bool {
    let start = Instant::now();
    unsafe {
        loop {
            // The handle stays open for the life of the process.
            let Ok(mutex) = CreateMutexW(None, true, w!("Local\\SurvivorsSatchel.Instance")) else { return true };
            if GetLastError() != ERROR_ALREADY_EXISTS {
                return true;
            }
            if start.elapsed() >= wait {
                break;
            }
            // Only the old Satchel's handle may keep the name alive between two looks.
            let _ = CloseHandle(mutex);
            thread::sleep(Duration::from_millis(500));
        }
        let message = match request {
            Request::Show => show_message(),
            Request::Play => play_message(),
        };
        let _ = PostMessageW(Some(HWND_BROADCAST), message, WPARAM(0), LPARAM(0));
    }
    false
}

// The hidden message window ---------------------------------------------------------------------

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static SENDING: AtomicBool = AtomicBool::new(false);
static SHUTDOWN_ASKED: AtomicBool = AtomicBool::new(false);
static HANDLERS: OnceLock<Handlers> = OnceLock::new();

struct Handlers {
    show: Box<dyn Fn() + Send + Sync>,
    play: Box<dyn Fn() + Send + Sync>,
    show_id: u32,
    play_id: u32,
}

const WM_HOLD: u32 = WM_APP + 1;
const WM_RELEASE: u32 = WM_APP + 2;

/// Starts the hidden top-level window that hears Windows shutting down, and a second Satchel
/// asking for the window or for Play.
pub fn start_message_window(show: impl Fn() + Send + Sync + 'static, play: impl Fn() + Send + Sync + 'static) {
    let _ = HANDLERS.set(Handlers {
        show: Box::new(show),
        play: Box::new(play),
        show_id: show_message(),
        play_id: play_message(),
    });
    std::thread::spawn(|| unsafe {
        let Ok(module) = GetModuleHandleW(PCWSTR::null()) else { return };
        let class = w!("SurvivorsSatchelMessages");
        let window_class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassW(&window_class);
        let created = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("Survivor's Satchel"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(module.into()),
            None,
        );
        let Ok(hwnd) = created else {
            log!("the message window could not be created: shutdown hold and second starts won't work");
            return;
        };
        WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    });
}

unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_QUERYENDSESSION => {
            if SENDING.load(Ordering::SeqCst) {
                SHUTDOWN_ASKED.store(true, Ordering::SeqCst);
                log!("Windows is shutting down while a send runs: holding it until the send is done");
                return LRESULT(0);
            }
            log!("Windows is shutting down");
            LRESULT(1)
        }
        WM_ENDSESSION => {
            if wparam.0 == 0 {
                SHUTDOWN_ASKED.store(false, Ordering::SeqCst);
            }
            LRESULT(0)
        }
        WM_HOLD => {
            let _ = unsafe { ShutdownBlockReasonCreate(hwnd, w!("Survivor's Satchel: saving your progress")) };
            LRESULT(0)
        }
        WM_RELEASE => {
            let _ = unsafe { ShutdownBlockReasonDestroy(hwnd) };
            LRESULT(0)
        }
        _ => {
            if let Some(handlers) = HANDLERS.get() {
                if message != 0 && message == handlers.show_id {
                    (handlers.show)();
                    return LRESULT(0);
                }
                if message != 0 && message == handlers.play_id {
                    (handlers.play)();
                    return LRESULT(0);
                }
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
    }
}

fn post(message: u32) {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut c_void)), message, WPARAM(0), LPARAM(0));
        }
    }
}

/// Holds Windows' shutdown while a send runs; the shutdown screen says "Survivor's Satchel:
/// saving your progress". If Windows asked to shut down meanwhile, the Satchel exits as soon as
/// the send is done, which lets the shutdown go on.
pub struct Hold;

impl Hold {
    pub fn start() -> Hold {
        SENDING.store(true, Ordering::SeqCst);
        post(WM_HOLD);
        Hold
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        SENDING.store(false, Ordering::SeqCst);
        post(WM_RELEASE);
        if SHUTDOWN_ASKED.load(Ordering::SeqCst) {
            log!("send done during a shutdown: exiting so Windows can go on");
            std::process::exit(0);
        }
    }
}

// The game -----------------------------------------------------------------------------------

/// Every running process: its id and exe name.
fn processes() -> Vec<(u32, String)> {
    let mut found = Vec::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return found };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                found.push((entry.th32ProcessID, String::from_utf16_lossy(&entry.szExeFile[..len])));
                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    found
}

fn image_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        result.ok()?;
        Some(PathBuf::from(OsString::from_wide(&buffer[..len as usize])))
    }
}

/// A folder as a lowercase prefix ending in a backslash, for comparisons.
fn folder_key(path: &Path) -> String {
    let mut key = path.to_string_lossy().to_lowercase().replace('/', "\\");
    if !key.ends_with('\\') {
        key.push('\\');
    }
    key
}

/// Watches for the game: any process running from the game folder counts, so the game's own
/// updater does too.
pub struct Scanner {
    paths: HashMap<u32, Option<PathBuf>>,
    own: u32,
}

impl Scanner {
    pub fn new() -> Scanner {
        Scanner { paths: HashMap::new(), own: std::process::id() }
    }

    pub fn game_running(&mut self, game_dir: &Path) -> bool {
        let folder = folder_key(game_dir);
        let running = processes();
        let alive: HashSet<u32> = running.iter().map(|(pid, _)| *pid).collect();
        self.paths.retain(|pid, _| alive.contains(pid));
        let mut found = false;
        for (pid, name) in running {
            if pid == 0 || pid == self.own {
                continue;
            }
            let path = self.paths.entry(pid).or_insert_with(|| image_path(pid));
            found |= match path {
                Some(path) => folder_key(path).starts_with(&folder),
                // A process the Satchel may not look into (elevated, another user): its name decides.
                None => name.eq_ignore_ascii_case(GAME_EXE),
            };
        }
        found
    }
}

/// Finds the game folder: a game running right now, a game beside the Satchel, the usual places.
/// Presses Esc in the game when it stops being the window in front (Alt-Tab, the Windows key, a
/// click on another window, the lock screen), so it pauses. A thread looks at which window is in
/// front ten times a second while the game runs and this is on; the keyboard is never read. The
/// key goes to the game's window as a message, so the game needn't be in front to get it.
pub struct AwayWatch {
    game: Arc<Mutex<Option<PathBuf>>>,
}

/// With no window in front this many looks in a row (the lock screen, a UAC prompt), the game
/// counts as left.
const AWAY_BLANK_LOOKS: u32 = 5;

impl AwayWatch {
    pub fn start() -> AwayWatch {
        let game = Arc::new(Mutex::new(None));
        let watched = Arc::clone(&game);
        if let Err(e) = thread::Builder::new().name("satchel-away".into()).spawn(move || away_loop(&watched)) {
            log!("the pause-when-away watcher didn't start: {e}");
        }
        AwayWatch { game }
    }

    /// Watches the game in `dir`; `None` stops.
    pub fn watch(&self, dir: Option<&Path>) {
        if let Ok(mut game) = self.game.lock()
            && game.as_deref() != dir
        {
            *game = dir.map(Path::to_path_buf);
        }
    }
}

fn away_loop(game: &Mutex<Option<PathBuf>>) {
    // The game's window last seen in front.
    let mut front: Option<HWND> = None;
    let mut ours: HashMap<u32, bool> = HashMap::new();
    let mut blank = 0;
    loop {
        thread::sleep(Duration::from_millis(100));
        let Some(dir) = game.lock().ok().and_then(|g| g.clone()) else {
            front = None;
            ours.clear();
            continue;
        };
        let window = unsafe { GetForegroundWindow() };
        if window.is_invalid() {
            blank += 1;
            if blank < AWAY_BLANK_LOOKS {
                continue;
            }
        } else {
            blank = 0;
        }
        let mut pid = 0u32;
        if !window.is_invalid() {
            unsafe { GetWindowThreadProcessId(window, Some(&mut pid as *mut u32)) };
        }
        let folder = folder_key(&dir);
        let is_game = pid != 0
            && *ours.entry(pid).or_insert_with(|| image_path(pid).is_some_and(|path| folder_key(&path).starts_with(&folder)));
        if is_game {
            front = Some(window);
        } else if let Some(left) = front.take() {
            press_esc(left);
            log!("the game stopped being the window in front: Esc sent to it, to pause");
        }
    }
}

/// Esc down and up, as messages to `window` (scan code 1, as a keyboard sends it).
fn press_esc(window: HWND) {
    const VK_ESCAPE: usize = 0x1B;
    const SCAN: isize = 0x01 << 16;
    // Key up: bit 30 (it was down) and bit 31 (it's going up).
    const UP: isize = 0xC000_0000;
    unsafe {
        let _ = PostMessageW(Some(window), WM_KEYDOWN, WPARAM(VK_ESCAPE), LPARAM(1 | SCAN));
        let _ = PostMessageW(Some(window), WM_KEYUP, WPARAM(VK_ESCAPE), LPARAM(1 | SCAN | UP));
    }
}

pub fn detect_game_dir() -> Option<PathBuf> {
    for (pid, name) in processes() {
        if name.eq_ignore_ascii_case(GAME_EXE)
            && let Some(dir) = image_path(pid).and_then(|exe| exe.parent().map(Path::to_path_buf))
        {
            return Some(dir);
        }
    }
    let mut places = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf)) {
        places.push(dir);
    }
    places.push(PathBuf::from(r"C:\games\AzerothSurvivors"));
    places.push(PathBuf::from(r"C:\Git\wenilla\benilla-config"));
    for variable in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(dir) = std::env::var_os(variable).map(PathBuf::from) {
            places.push(dir.join("AzerothSurvivors"));
            places.push(dir.join("Azeroth Survivors"));
        }
    }
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        for sub in ["Desktop", "Downloads", "Games"] {
            places.push(home.join(sub).join("AzerothSurvivors"));
        }
    }
    places.into_iter().find(|dir| dir.join(GAME_EXE).is_file())
}

pub fn launch_game(game_dir: &Path) -> io::Result<()> {
    Command::new(game_dir.join(GAME_EXE)).current_dir(game_dir).spawn().map(drop)
}

// The Satchel updating itself ---------------------------------------------------------------------

/// The flag a Satchel started by an update gets, with the id of the process it replaces.
pub const AFTER_UPDATE: &str = "--after-update";

/// The version an exe carries in its version resource ("0.4.1"), e.g. a downloaded update's;
/// `None` when it has none or can't be read.
pub fn exe_version(path: &Path) -> Option<String> {
    let name = wide(path.as_os_str());
    unsafe {
        let size = GetFileVersionInfoSizeW(PCWSTR(name.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(name.as_ptr()), None, size, data.as_mut_ptr().cast()).ok()?;
        let mut value: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        // The string keeps a pre-release tag the numbers can't.
        if VerQueryValueW(data.as_ptr().cast(), w!("\\StringFileInfo\\040904b0\\ProductVersion"), &mut value, &mut len).as_bool()
            && !value.is_null()
            && len > 0
        {
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(value as *const u16, len as usize));
            let text = text.trim_end_matches('\0').trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
        if VerQueryValueW(data.as_ptr().cast(), w!("\\"), &mut value, &mut len).as_bool()
            && !value.is_null()
            && len as usize >= std::mem::size_of::<VS_FIXEDFILEINFO>()
        {
            let info = std::ptr::read_unaligned(value as *const VS_FIXEDFILEINFO);
            if info.dwSignature == 0xFEEF_04BD {
                return Some(format!("{}.{}.{}", info.dwProductVersionMS >> 16, info.dwProductVersionMS & 0xFFFF, info.dwProductVersionLS >> 16));
            }
        }
    }
    None
}

/// Puts the exe at `new` where the running one (`me`) is: the running exe moves aside first
/// (Windows lets a running exe be renamed, not replaced), then the new one takes its name. When
/// the second move fails, the running exe is put back.
pub fn swap_in_exe(me: &Path, new: &Path, aside: &Path) -> io::Result<()> {
    let (me_w, new_w, aside_w) = (wide(me.as_os_str()), wide(new.as_os_str()), wide(aside.as_os_str()));
    let flags = MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH;
    unsafe {
        MoveFileExW(PCWSTR(me_w.as_ptr()), PCWSTR(aside_w.as_ptr()), flags).map_err(|e| io::Error::other(format!("the running Satchel couldn't be moved aside: {e}")))?;
        if let Err(e) = MoveFileExW(PCWSTR(new_w.as_ptr()), PCWSTR(me_w.as_ptr()), flags) {
            if let Err(back) = MoveFileExW(PCWSTR(aside_w.as_ptr()), PCWSTR(me_w.as_ptr()), flags) {
                log!("the Satchel couldn't be put back from {}: {back}", aside.display());
            }
            return Err(io::Error::other(format!("the new Satchel couldn't take its place: {e}")));
        }
    }
    Ok(())
}

/// Starts the Satchel at `exe` to replace this process; with `play`, it plays once it's up.
pub fn start_updated(exe: &Path, play: bool) -> io::Result<std::process::Child> {
    let folder = exe.parent().unwrap_or(Path::new("."));
    let mut command = Command::new(exe);
    command.arg(AFTER_UPDATE).arg(std::process::id().to_string()).current_dir(folder);
    if play {
        command.arg("--play");
    }
    command.spawn()
}

/// Waits, at most `wait`, for the process `pid` to end: a Satchel started by an update lets the
/// one it replaces close first (one Satchel per PC).
pub fn wait_for_exit(pid: u32, wait: Duration) {
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(process, u32::try_from(wait.as_millis()).unwrap_or(u32::MAX));
            let _ = CloseHandle(process);
        }
    }
}

// Windows settings ------------------------------------------------------------------------------

fn win32_result(status: windows::Win32::Foundation::WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(status.0 as i32)) }
}

/// Start with Windows, in the tray (the user's Run key; no admin rights needed).
pub fn set_start_with_windows(on: bool) -> io::Result<()> {
    let key = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = w!("SurvivorsSatchel");
    unsafe {
        if on {
            let exe = std::env::current_exe()?;
            let command = wide(OsStr::new(&format!("\"{}\" --tray", exe.display())));
            win32_result(RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key,
                name,
                REG_SZ.0,
                Some(command.as_ptr() as *const c_void),
                (command.len() * 2) as u32,
            ))
        } else {
            match RegDeleteKeyValueW(HKEY_CURRENT_USER, key, name) {
                status if status == ERROR_FILE_NOT_FOUND => Ok(()),
                status => win32_result(status),
            }
        }
    }
}

fn desktop() -> io::Result<PathBuf> {
    unsafe {
        let raw = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).map_err(io::Error::other)?;
        let path = PathBuf::from(OsString::from_wide(raw.as_wide()));
        CoTaskMemFree(Some(raw.0 as *const c_void));
        Ok(path)
    }
}

/// The desktop shortcut "Play Azeroth Survivors": the Satchel with `--play`, the game's icon.
pub fn set_play_shortcut(on: bool, icon_from: Option<&Path>) -> io::Result<()> {
    let link = desktop()?.join("Play Azeroth Survivors.lnk");
    if !on {
        return match std::fs::remove_file(&link) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    unsafe {
        let shell_link: IShellLinkW =
            CoCreateInstance(&ShellLink, None::<&IUnknown>, CLSCTX_INPROC_SERVER).map_err(io::Error::other)?;
        shell_link.SetPath(&HSTRING::from(exe.as_path())).map_err(io::Error::other)?;
        shell_link.SetArguments(w!("--play")).map_err(io::Error::other)?;
        shell_link
            .SetDescription(w!("Brings in your newest progress, starts the game, sends it back after"))
            .map_err(io::Error::other)?;
        if let Some(dir) = exe.parent() {
            shell_link.SetWorkingDirectory(&HSTRING::from(dir)).map_err(io::Error::other)?;
        }
        if let Some(icon) = icon_from {
            shell_link.SetIconLocation(&HSTRING::from(icon), 0).map_err(io::Error::other)?;
        }
        let file: IPersistFile = shell_link.cast().map_err(io::Error::other)?;
        file.Save(&HSTRING::from(link.as_path()), true).map_err(io::Error::other)?;
    }
    Ok(())
}

/// COM for this thread (the shortcut and notifications need it).
pub fn com_init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Registers the name Windows shows on the Satchel's notifications.
pub fn register_notifications() {
    let name = wide(OsStr::new("Survivor's Satchel"));
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Classes\\AppUserModelId\\SurvivorsSatchel"),
            w!("DisplayName"),
            REG_SZ.0,
            Some(name.as_ptr() as *const c_void),
            (name.len() * 2) as u32,
        );
    }
}

pub fn notify(title: &str, text: &str) {
    let shown = Toast::new(APP_ID).title(title).text1(text).duration(ToastDuration::Short).sound(None).show();
    if let Err(e) = shown {
        log!("notification not shown ({title}): {e}");
    }
}

/// "F:\games\Saves" on a mapped network drive becomes "\\nas\share\games\Saves", which Windows
/// reaches even before the drive letter is reconnected after login. Other paths stay as they are.
pub fn network_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return path.to_path_buf();
    }
    let drive = wide(OsStr::new(&text[..2]));
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    let status = unsafe { WNetGetConnectionW(PCWSTR(drive.as_ptr()), Some(PWSTR(buffer.as_mut_ptr())), &mut len) };
    if status != ERROR_SUCCESS {
        return path.to_path_buf();
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    let remote = String::from_utf16_lossy(&buffer[..end]);
    PathBuf::from(format!("{}{}", remote.trim_end_matches('\\'), &text[2..]))
}

/// The desktop area not covered by the taskbar: left, top, right, bottom.
pub fn work_area() -> Option<(i32, i32, i32, i32)> {
    let mut rect = RECT::default();
    unsafe {
        SystemParametersInfoW(
            SPI_GETWORKAREA,
            0,
            Some(&mut rect as *mut RECT as *mut c_void),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .ok()?;
    }
    Some((rect.left, rect.top, rect.right, rect.bottom))
}

/// Brings a Satchel window to the front, restored if it was minimized. Windows allows this right
/// after the user acted on the Satchel (a tray menu click, a second start).
pub fn bring_to_front(handle: &slint::WindowHandle) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};
    let Ok(handle) = handle.window_handle() else { return };
    if let RawWindowHandle::Win32(win32) = handle.as_raw() {
        let hwnd = HWND(win32.hwnd.get() as *mut c_void);
        unsafe {
            if IsIconic(hwnd).as_bool() {
                let _ = ShowWindow(hwnd, SW_RESTORE);
            }
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub fn open_folder(path: &Path) {
    let _ = Command::new("explorer.exe").arg(path).spawn();
}

pub fn show_in_folder(file: &Path) {
    let _ = Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", file.display())).spawn();
}

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "This PC".into())
}

// Addons: the Recycle Bin, links, the zip picker ---------------------------------------------

/// Moves a file or folder to the Recycle Bin, without asking (the Satchel asked already).
pub fn recycle(path: &Path) -> io::Result<()> {
    use windows::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
    };
    // A list of paths, each ending in a NUL, the list in one more.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut operation = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_NOERRORUI.0 | FOF_SILENT.0) as u16,
        ..Default::default()
    };
    let status = unsafe { SHFileOperationW(&mut operation) };
    if status != 0 || operation.fAnyOperationsAborted.as_bool() {
        return Err(io::Error::other(format!("Windows didn't move {} to the Recycle Bin (code {status})", path.display())));
    }
    Ok(())
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    if !url.starts_with("https://") {
        return;
    }
    unsafe {
        let _ = ShellExecuteW(None, w!("open"), &HSTRING::from(url), None, None, SW_SHOWNORMAL);
    }
}

/// The game's website, where it is downloaded.
pub const GAME_SITE: &str = "https://wow.aethus.pl";

/// Whether Windows' apps are dark (Settings → Personalization → Colors, "Choose your app mode"):
/// the theme "System" follows it. Light when it can't be read, as Windows comes.
pub fn apps_use_dark() -> bool {
    use windows::Win32::System::Registry::{RRF_RT_REG_DWORD, RegGetValueW};
    let mut light: u32 = 1;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut light as *mut u32 as *mut c_void),
            Some(&mut size),
        )
    };
    status == ERROR_SUCCESS && light == 0
}

/// The window's title bar dark or light, as its colours are: Windows draws it, and Slint sets it
/// only from Windows' own app mode.
pub fn dark_title_bar(handle: &slint::WindowHandle, dark: bool) {
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let Some(hwnd) = native_handle(handle) else { return };
    let value: i32 = dark.into();
    unsafe {
        let _ = DwmSetWindowAttribute(
            HWND(hwnd as *mut c_void),
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// The window's name on the taskbar, and what a pin of it starts: the Satchel's window. Without
/// it Windows pins the command line the Satchel was started with, and the Play shortcut starts it
/// with `--play` (Microsoft, "Application User Model IDs": a window with its own id can name its
/// relaunch command).
pub fn app_identity(handle: &slint::WindowHandle) {
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand, PKEY_AppUserModel_RelaunchDisplayNameResource,
        PKEY_AppUserModel_RelaunchIconResource,
    };
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
    let Some(hwnd) = native_handle(handle) else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    let set = || -> windows::core::Result<()> {
        unsafe {
            let store: IPropertyStore = SHGetPropertyStoreForWindow(HWND(hwnd as *mut c_void))?;
            set_text(&store, &PKEY_AppUserModel_ID, APP_ID)?;
            set_text(&store, &PKEY_AppUserModel_RelaunchCommand, &format!("\"{}\"", exe.display()))?;
            set_text(&store, &PKEY_AppUserModel_RelaunchDisplayNameResource, "Survivor's Satchel")?;
            set_text(&store, &PKEY_AppUserModel_RelaunchIconResource, &format!("{},0", exe.display()))?;
            store.Commit()
        }
    };
    if let Err(e) = set() {
        log!("the taskbar's name for the window wasn't set: {e}");
    }
}

/// One text property: a VT_LPWSTR value, its string from the COM allocator so that clearing the
/// value frees it.
unsafe fn set_text(
    store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore,
    key: &windows::Win32::Foundation::PROPERTYKEY,
    text: &str,
) -> windows::core::Result<()> {
    use std::mem::ManuallyDrop;
    use windows::Win32::System::Com::StructuredStorage::{PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0, PropVariantClear};
    use windows::Win32::System::Variant::VT_LPWSTR;
    use windows::Win32::UI::Shell::SHStrDupW;
    unsafe {
        let value = SHStrDupW(&HSTRING::from(text))?;
        let mut variant = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_LPWSTR,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { pwszVal: value },
                }),
            },
        };
        let result = store.SetValue(key, &variant);
        let _ = PropVariantClear(&mut variant);
        result
    }
}

/// The native window behind a Slint window, as a number that can cross threads.
pub fn native_handle(handle: &slint::WindowHandle) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(win32) => Some(win32.hwnd.get()),
        _ => None,
    }
}

/// Windows' file picker for an addon zip, in front of the Satchel's window (`owner`). Blocks
/// until the player picks or cancels, so it runs on a thread of its own.
pub fn pick_zip(owner: Option<isize>) -> Option<PathBuf> {
    use windows::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    com_init();
    let mut file = vec![0u16; 4096];
    // Pairs of a label and a pattern, each ending in a NUL, the list in one more.
    let filter: Vec<u16> = ["Addon zip files (*.zip)", "*.zip", "All files", "*.*"]
        .iter()
        .flat_map(|part| OsStr::new(part).encode_wide().chain(Some(0)))
        .chain(Some(0))
        .collect();
    let title = wide(OsStr::new("Import an addon zip file"));
    let mut dialog = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: HWND(owner.unwrap_or(0) as *mut c_void),
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: PCWSTR(title.as_ptr()),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    if !unsafe { GetOpenFileNameW(&mut dialog) }.as_bool() {
        return None;
    }
    let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(PathBuf::from(OsString::from_wide(&file[..end])))
}
