use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct WindowTarget {
    pub hwnd: usize,
    pub title: String,
    pub process_id: u32,
}

#[cfg(windows)]
pub fn enumerate_windows() -> Vec<WindowTarget> {
    use windows_sys::Win32::{
        Foundation::LPARAM, System::Threading::GetCurrentProcessId,
        UI::WindowsAndMessaging::EnumWindows,
    };

    let mut windows = Vec::<WindowTarget>::new();
    let current_process_id = unsafe { GetCurrentProcessId() };
    let context = &mut windows as *mut Vec<WindowTarget> as LPARAM;
    unsafe {
        EnumWindows(Some(collect_window), context);
    }
    windows.retain(|window| window.process_id != current_process_id);
    windows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    windows
}

#[cfg(windows)]
unsafe extern "system" fn collect_window(
    hwnd: windows_sys::Win32::Foundation::HWND,
    context: windows_sys::Win32::Foundation::LPARAM,
) -> windows_sys::core::BOOL {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    };

    if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
        return 1;
    }
    let title_length = GetWindowTextLengthW(hwnd);
    if title_length <= 0 || title_length > 16_384 {
        return 1;
    }
    let mut title = vec![0u16; title_length as usize + 1];
    let copied = GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32);
    if copied <= 0 {
        return 1;
    }
    title.truncate(copied as usize);
    let mut process_id = 0u32;
    GetWindowThreadProcessId(hwnd, &mut process_id);
    let windows = &mut *(context as *mut Vec<WindowTarget>);
    windows.push(WindowTarget {
        hwnd: hwnd as usize,
        title: String::from_utf16_lossy(&title),
        process_id,
    });
    1
}

#[cfg(windows)]
pub fn find_window(hwnd: usize) -> Option<WindowTarget> {
    enumerate_windows()
        .into_iter()
        .find(|window| window.hwnd == hwnd)
}

#[cfg(not(windows))]
pub fn enumerate_windows() -> Vec<WindowTarget> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn find_window(_hwnd: usize) -> Option<WindowTarget> {
    None
}
