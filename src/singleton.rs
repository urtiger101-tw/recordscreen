use std::{io, ptr::null, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, SetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
    },
    System::Threading::{CreateEventW, SetEvent, WaitForSingleObject},
    UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};

// Both the lock and its notification are scoped to the Windows session. Keep the name stable
// across versions so launching a newer copy can also wake the existing window.
const EVENT_NAME: &str = r"Local\RecordScreen.Gui.9A6354FA-C073-408A-931F-E2838B341125";

pub(super) struct Instance(HANDLE);

impl Instance {
    /// Return the instance guard, or notify the original window and return `None`.
    pub(super) fn launch() -> io::Result<Option<Self>> {
        Self::launch_named(EVENT_NAME)
    }

    fn launch_named(name: &str) -> io::Result<Option<Self>> {
        let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: the name is NUL-terminated and valid for the call. Null security attributes
        // give the event the default security descriptor and a non-inheritable handle.
        unsafe {
            SetLastError(0);
            let handle = CreateEventW(null(), 0, 0, name.as_ptr());
            let last_error = GetLastError();
            if handle.is_null() {
                return Err(io::Error::from_raw_os_error(last_error as i32));
            }
            let instance = Self(handle);
            if last_error == ERROR_ALREADY_EXISTS {
                if SetEvent(instance.0) == 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(None);
            }
            Ok(Some(instance))
        }
    }

    pub(super) fn take_restore_request(&self) -> bool {
        // SAFETY: the guard owns this handle and cannot be dropped during the wait. A zero
        // timeout keeps the UI responsive. The auto-reset event consumes the request.
        unsafe { WaitForSingleObject(self.0, 0) == WAIT_OBJECT_0 }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: this guard owns the handle returned by CreateEventW.
        unsafe { CloseHandle(self.0) };
    }
}

pub(super) fn show_error(message: &str) {
    let message: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    // SAFETY: both strings are NUL-terminated and valid for this synchronous call.
    unsafe {
        MessageBoxW(
            null_mut(),
            message.as_ptr(),
            windows_sys::w!("RecordScreen"),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::Command,
        sync::{Arc, Barrier},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn fixture_name() -> String {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        format!(
            "Local\\RecordScreen.Gui.Test.{}.{}",
            std::process::id(),
            now.as_nanos()
        )
    }

    #[test]
    fn competing_launches_elect_one_owner() {
        let name = fixture_name();
        let barrier = Arc::new(Barrier::new(4));
        let owners = thread::scope(|scope| {
            let contenders: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        let launch = Instance::launch_named(&name);
                        barrier.wait(); // Keep the winning handle alive until everyone has opened it.
                        launch.unwrap().is_some()
                    })
                })
                .collect();
            contenders
                .into_iter()
                .map(|contender| contender.join().unwrap())
                .filter(|owner| *owner)
                .count()
        });
        assert_eq!(owners, 1);
        assert!(Instance::launch_named(&name).unwrap().is_some());
    }

    #[test]
    fn second_process_wakes_owner_and_later_launch_can_take_over() {
        let name = fixture_name();
        let owner = Instance::launch_named(&name).unwrap().unwrap();
        assert!(!owner.take_restore_request());
        let launch_child = |role| {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "singleton::tests::fixture_process"])
                .env("RECORDSCREEN_SINGLETON_TEST_EVENT", &name)
                .env("RECORDSCREEN_SINGLETON_TEST_ROLE", role)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        };
        launch_child("secondary");
        assert!(owner.take_restore_request());
        assert!(!owner.take_restore_request());
        drop(owner);
        launch_child("primary");
    }

    #[test]
    fn fixture_process() {
        let Ok(name) = std::env::var("RECORDSCREEN_SINGLETON_TEST_EVENT") else {
            return;
        };
        assert!(name.starts_with("Local\\RecordScreen.Gui.Test."));
        let launch = Instance::launch_named(&name).unwrap();
        assert_eq!(
            launch.is_some(),
            std::env::var("RECORDSCREEN_SINGLETON_TEST_ROLE").unwrap() == "primary"
        );
    }
}
