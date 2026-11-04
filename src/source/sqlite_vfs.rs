//! SQLite's stock WAL opener requests CREATE even for read-only databases.
//! Keep the platform VFS and its locking/SHM protocol, but forbid WAL/journal creation.
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
        // SAFETY: SQLite owns the default VFS for process lifetime; no code here
        // unregisters it. Check for null before copying its initialized structure.
        // The leaked copy and static C name outlive registration and all callbacks;
        // retain the parent's callback/data pointers except our xOpen guard, and
        // szOsFile/iVersion so SQLite allocates a file buffer matching the parent.
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
    // SAFETY: registration stores a non-null parent before installing this callback;
    // acquire/release publishes it. SQLite supplies xOpen arguments valid for this
    // call, which we forward unchanged to the live parent except for the flags.
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
