//! SQLite's stock WAL opener requests CREATE even for read-only databases.
//! Keep the platform VFS and its locking/SHM protocol, but forbid sidecar creation.
use rusqlite::{Connection, OpenFlags, ffi};
use std::{
    ffi::{c_char, c_int},
    ptr,
    sync::{
        OnceLock,
        atomic::{AtomicPtr, Ordering},
    },
};

static PARENT: AtomicPtr<ffi::sqlite3_vfs> = AtomicPtr::new(ptr::null_mut());
static REGISTERED: OnceLock<c_int> = OnceLock::new();
const NAME: &str = "sboard_readonly";

pub(super) fn open(uri: &str, flags: OpenFlags) -> rusqlite::Result<Connection> {
    let code = *REGISTERED.get_or_init(|| {
        // SQLite owns the parent for process lifetime. Register a leaked copy,
        // retaining every platform callback and pAppData except our xOpen guard.
        unsafe {
            let parent = ffi::sqlite3_vfs_find(ptr::null());
            if parent.is_null() {
                return ffi::SQLITE_CANTOPEN;
            }
            PARENT.store(parent, Ordering::Release);
            let mut vfs = Box::new(*parent);
            vfs.pNext = ptr::null_mut();
            vfs.zName = c"sboard_readonly".as_ptr();
            vfs.xOpen = Some(open_existing_sidecar);
            ffi::sqlite3_vfs_register(Box::into_raw(vfs), 0)
        }
    });
    if code != ffi::SQLITE_OK {
        return Err(rusqlite::Error::SqliteFailure(
            ffi::Error::new(code),
            Some("cannot register read-only SQLite VFS".into()),
        ));
    }
    Connection::open_with_flags_and_vfs(uri, flags, NAME)
}

unsafe extern "C" fn open_existing_sidecar(
    _: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    mut flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    if flags
        & (ffi::SQLITE_OPEN_WAL | ffi::SQLITE_OPEN_MAIN_JOURNAL | ffi::SQLITE_OPEN_SUPER_JOURNAL)
        != 0
    {
        flags &= !(ffi::SQLITE_OPEN_CREATE
            | ffi::SQLITE_OPEN_EXCLUSIVE
            | ffi::SQLITE_OPEN_DELETEONCLOSE);
        #[cfg(test)]
        BEFORE_SIDECAR_OPEN.with(|hook| {
            let callback = hook.borrow_mut().take();
            if let Some(callback) = callback {
                callback();
            }
        });
    }
    let parent = PARENT.load(Ordering::Acquire);
    // Registration precedes every callback. SQLite supplies valid C arguments;
    // dispatch through the unchanged parent with CREATE atomically disabled.
    unsafe {
        match (*parent).xOpen {
            Some(open) => open(parent, name, file, flags, out_flags),
            None => ffi::SQLITE_CANTOPEN,
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(super) static BEFORE_SIDECAR_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
