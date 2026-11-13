//! Verified Full snapshots owned by a long-lived supervisor.
//!
//! Mounting pays the inventory and copy costs once. The private copy is unlinked
//! and its only writable descriptor is closed before requests can use it. Workers
//! inherit the read-only descriptor; neither SQLite nor a worker reopens a source
//! pathname. The small VFS below reads that descriptor with `pread`, and delegates
//! SQLite's temporary files to its normal VFS. It never provides writes or mmap for
//! the main database.
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, ffi};
use serde_json::Value;
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    fs::{self, File},
    io,
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    ptr,
    sync::OnceLock,
};

const LOCATOR: &str = "ontokiln-fd:";
const VFS_NAME: &CStr = c"ontokiln-fd-v1";

pub struct FullSnapshot {
    source: PathBuf,
    manifest: Value,
    manifest_digest: String,
    database: File,
    bytes: u64,
}

impl FullSnapshot {
    pub fn mount(source: &Path, max_bytes: u64) -> Result<Self> {
        ensure!(max_bytes > 0, "BUDGET_EXHAUSTED|zero mount disk allowance");
        let source = std::path::absolute(source)?;
        let database_path = source.join("facts.sqlite");
        // Reject an obviously oversized input before inventory hashing. This is
        // only a cheap admission check; verified metadata and the copy itself
        // still enforce the allowance if the source changes concurrently.
        let metadata = fs::symlink_metadata(&database_path)
            .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
        ensure!(
            metadata.is_file(),
            "INVALID_DATASET|expected a regular non-symlink database"
        );
        ensure!(
            metadata.len() <= max_bytes,
            "BUDGET_EXHAUSTED|snapshot exceeds mount disk allowance"
        );
        let (manifest, manifest_digest) =
            crate::full_artifact::verify_with_manifest_digest(&source)
                .map_err(|e| anyhow::anyhow!("INVALID_DATASET|{e}"))?;
        ensure!(
            matches!(manifest["kind"].as_str(), Some("compiled" | "reasoned"))
                && (manifest["kind"] != "reasoned" || manifest["fixpoint_reached"] == true),
            "INVALID_DATASET|snapshot has no complete supported view"
        );
        let bytes = manifest["files"]["facts.sqlite"]["bytes"]
            .as_u64()
            .context("INVALID_DATASET|missing snapshot size")?;
        ensure!(
            bytes <= max_bytes,
            "BUDGET_EXHAUSTED|snapshot exceeds mount disk allowance"
        );
        let digest = manifest["files"]["facts.sqlite"]["sha256"]
            .as_str()
            .context("INVALID_DATASET|missing snapshot digest")?;
        let temporary = tempfile::Builder::new()
            .prefix("full-snapshot-")
            .tempfile()?;
        let (mut writer, path) = temporary.into_parts();
        let database = File::open(&path)?;
        // Unlink at zero bytes, before the potentially long copy. Signals or an
        // abrupt process exit during mounting cannot leave database bytes behind.
        path.close()?;
        crate::full_artifact::freeze_into(&database_path, &mut writer, digest, max_bytes).map_err(
            |error| {
                if error.to_string().starts_with("BUDGET_EXHAUSTED|") {
                    error
                } else {
                    anyhow::anyhow!("INVALID_DATASET|{error}")
                }
            },
        )?;
        drop(writer);
        validate_descriptor(database.as_raw_fd(), bytes)?;
        Ok(Self {
            source,
            manifest,
            manifest_digest,
            database,
            bytes,
        })
    }

    pub fn source_path(&self) -> &Path {
        &self.source
    }

    pub fn manifest(&self) -> &Value {
        &self.manifest
    }

    pub fn manifest_sha256(&self) -> &str {
        &self.manifest_digest
    }

    pub fn database_bytes(&self) -> u64 {
        self.bytes
    }

    pub fn descriptor(&self) -> c_int {
        self.database.as_raw_fd()
    }

    pub fn database_locator(&self) -> String {
        format!("{LOCATOR}{}", self.descriptor())
    }

    /// The caller must keep this snapshot alive until the command has spawned.
    pub fn inherit(&self, command: &mut Command) -> Result<()> {
        let fd = self.descriptor();
        validate_descriptor(fd, self.bytes)?;
        // SAFETY: fcntl is async-signal-safe. This changes only the child's FD.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

pub fn validate_descriptor(fd: c_int, expected_bytes: u64) -> Result<()> {
    let bytes = descriptor_bytes(fd)?;
    ensure!(
        bytes == expected_bytes,
        "INVALID_DATASET|mounted snapshot size mismatch"
    );
    Ok(())
}

fn descriptor_bytes(fd: c_int) -> Result<u64> {
    ensure!(fd > 2, "INVALID_DATASET|invalid snapshot descriptor");
    // SAFETY: F_GETFL only inspects the descriptor and does not take ownership.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    ensure!(
        flags >= 0 && flags & libc::O_ACCMODE == libc::O_RDONLY,
        "INVALID_DATASET|snapshot descriptor must be read-only"
    );
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: fstat reports EBADF for an invalid descriptor and initializes stat
    // only on success. No Rust ownership is constructed from the raw descriptor.
    let status = unsafe { libc::fstat(fd, stat.as_mut_ptr()) };
    if status != 0 {
        return Err(io::Error::last_os_error()).context("INVALID_DATASET|snapshot descriptor");
    }
    let stat = unsafe { stat.assume_init() };
    ensure!(
        stat.st_mode & libc::S_IFMT == libc::S_IFREG && stat.st_nlink == 0 && stat.st_size >= 0,
        "INVALID_DATASET|expected anonymous private snapshot"
    );
    Ok(stat.st_size as u64)
}

/// Open either a normal legacy read-only path or a supervisor-owned FD locator.
/// FD locators are internal transport, never an alternative artifact identifier.
pub fn open_readonly(path: &Path) -> Result<Connection> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Some(fd) = path.to_str().and_then(|s| s.strip_prefix(LOCATOR)) {
        let fd = fd
            .parse::<c_int>()
            .context("invalid mounted snapshot locator")?;
        descriptor_bytes(fd)?;
        register_vfs()?;
        Ok(Connection::open_with_flags_and_vfs(path, flags, VFS_NAME)?)
    } else {
        Ok(Connection::open_with_flags(path, flags)?)
    }
}

#[repr(C)]
struct SnapshotFile {
    base: ffi::sqlite3_file,
    fd: c_int,
}

static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(file_size),
    xLock: Some(lock),
    xUnlock: Some(lock),
    xCheckReservedLock: Some(reserved_lock),
    xFileControl: Some(file_control),
    xSectorSize: Some(sector_size),
    xDeviceCharacteristics: Some(device_characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

fn register_vfs() -> Result<()> {
    static REGISTERED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            // SAFETY: SQLite initialization is thread-safe. The registration and
            // leaked VFS live for the process, like SQLite's built-in VFS objects.
            unsafe {
                if ffi::sqlite3_initialize() != ffi::SQLITE_OK {
                    return Err("SQLite initialization failed".into());
                }
                let parent = ffi::sqlite3_vfs_find(ptr::null());
                if parent.is_null() {
                    return Err("SQLite has no default VFS".into());
                }
                let vfs = Box::new(ffi::sqlite3_vfs {
                    iVersion: 1,
                    szOsFile: (*parent).szOsFile.max(size_of::<SnapshotFile>() as c_int),
                    mxPathname: (*parent).mxPathname,
                    pNext: ptr::null_mut(),
                    zName: VFS_NAME.as_ptr(),
                    pAppData: parent.cast(),
                    xOpen: Some(open),
                    xDelete: Some(delete),
                    xAccess: Some(access),
                    xFullPathname: Some(full_pathname),
                    xDlOpen: Some(dl_open),
                    xDlError: Some(dl_error),
                    xDlSym: Some(dl_symbol),
                    xDlClose: Some(dl_close),
                    xRandomness: Some(randomness),
                    xSleep: Some(sleep),
                    xCurrentTime: Some(current_time),
                    xGetLastError: Some(last_error),
                    xCurrentTimeInt64: None,
                    xSetSystemCall: None,
                    xGetSystemCall: None,
                    xNextSystemCall: None,
                });
                let vfs = Box::into_raw(vfs);
                let status = ffi::sqlite3_vfs_register(vfs, 0);
                if status != ffi::SQLITE_OK {
                    drop(Box::from_raw(vfs));
                    return Err(format!("SQLite snapshot VFS registration failed: {status}"));
                }
                Ok(())
            }
        })
        .as_ref()
        .map_err(|message| anyhow::anyhow!("{message}"))
        .copied()
}

// All callbacks receive SQLite-owned buffers with the sizes promised by its VFS
// ABI. They return error codes, never unwind or transfer the caller's descriptor.
unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    out_flags: *mut c_int,
) -> c_int {
    unsafe {
        (*file).pMethods = ptr::null();
        if flags & ffi::SQLITE_OPEN_MAIN_DB == 0 {
            if is_locator(name) {
                return ffi::SQLITE_READONLY;
            }
            let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
            return (*parent).xOpen.unwrap()(parent, name, file, flags, out_flags);
        }
        if flags & (ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE) != 0 || name.is_null() {
            return ffi::SQLITE_READONLY;
        }
        let fd = CStr::from_ptr(name)
            .to_str()
            .ok()
            .and_then(|s| s.strip_prefix(LOCATOR))
            .and_then(|s| s.parse::<c_int>().ok());
        let Some(fd) = fd else {
            return ffi::SQLITE_CANTOPEN;
        };
        let copy = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3);
        if copy < 0 {
            return ffi::SQLITE_CANTOPEN;
        }
        if descriptor_bytes(copy).is_err() {
            libc::close(copy);
            return ffi::SQLITE_CANTOPEN;
        }
        ptr::write(
            file.cast::<SnapshotFile>(),
            SnapshotFile {
                base: ffi::sqlite3_file { pMethods: &METHODS },
                fd: copy,
            },
        );
        if !out_flags.is_null() {
            *out_flags = ffi::SQLITE_OPEN_READONLY;
        }
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let file = &mut *file.cast::<SnapshotFile>();
        let status = libc::close(file.fd);
        file.fd = -1;
        file.base.pMethods = ptr::null();
        if status == 0 {
            ffi::SQLITE_OK
        } else {
            ffi::SQLITE_IOERR_CLOSE
        }
    }
}

unsafe extern "C" fn read(
    file: *mut ffi::sqlite3_file,
    buffer: *mut c_void,
    amount: c_int,
    offset: i64,
) -> c_int {
    if amount < 0 || offset < 0 || offset.checked_add(i64::from(amount)).is_none() {
        return ffi::SQLITE_IOERR_READ;
    }
    let mut done = 0usize;
    while done < amount as usize {
        // SAFETY: SQLite supplies an `amount` byte output buffer; pread does not
        // share or change a file offset between connections or worker processes.
        let size = unsafe {
            libc::pread(
                (*file.cast::<SnapshotFile>()).fd,
                buffer.cast::<u8>().add(done).cast(),
                amount as usize - done,
                offset + done as i64,
            )
        };
        if size < 0 {
            if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return ffi::SQLITE_IOERR_READ;
        }
        if size == 0 {
            // SQLite requires unread bytes to be zeroed on every short read.
            unsafe { ptr::write_bytes(buffer.cast::<u8>().add(done), 0, amount as usize - done) };
            return ffi::SQLITE_IOERR_SHORT_READ;
        }
        done += size as usize;
    }
    ffi::SQLITE_OK
}

unsafe extern "C" fn write(_: *mut ffi::sqlite3_file, _: *const c_void, _: c_int, _: i64) -> c_int {
    ffi::SQLITE_READONLY
}
unsafe extern "C" fn truncate(_: *mut ffi::sqlite3_file, _: i64) -> c_int {
    ffi::SQLITE_READONLY
}
unsafe extern "C" fn sync(_: *mut ffi::sqlite3_file, _: c_int) -> c_int {
    ffi::SQLITE_OK
}
unsafe extern "C" fn file_size(file: *mut ffi::sqlite3_file, size: *mut i64) -> c_int {
    unsafe {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if libc::fstat((*file.cast::<SnapshotFile>()).fd, stat.as_mut_ptr()) != 0 {
            return ffi::SQLITE_IOERR_FSTAT;
        }
        *size = stat.assume_init().st_size;
        ffi::SQLITE_OK
    }
}
unsafe extern "C" fn lock(_: *mut ffi::sqlite3_file, _: c_int) -> c_int {
    ffi::SQLITE_OK
}
unsafe extern "C" fn reserved_lock(_: *mut ffi::sqlite3_file, result: *mut c_int) -> c_int {
    unsafe { *result = 0 };
    ffi::SQLITE_OK
}
unsafe extern "C" fn file_control(
    _: *mut ffi::sqlite3_file,
    operation: c_int,
    arg: *mut c_void,
) -> c_int {
    if operation == ffi::SQLITE_FCNTL_HAS_MOVED {
        unsafe { *arg.cast::<c_int>() = 0 };
        ffi::SQLITE_OK
    } else {
        ffi::SQLITE_NOTFOUND
    }
}
unsafe extern "C" fn sector_size(_: *mut ffi::sqlite3_file) -> c_int {
    4096
}
unsafe extern "C" fn device_characteristics(_: *mut ffi::sqlite3_file) -> c_int {
    ffi::SQLITE_IOCAP_IMMUTABLE
}

unsafe fn is_locator(name: *const c_char) -> bool {
    !name.is_null()
        && unsafe { CStr::from_ptr(name) }
            .to_bytes()
            .starts_with(LOCATOR.as_bytes())
}

unsafe extern "C" fn full_pathname(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    capacity: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        if is_locator(name) {
            let bytes = CStr::from_ptr(name).to_bytes_with_nul();
            if capacity <= 0 || bytes.len() > capacity as usize {
                return ffi::SQLITE_CANTOPEN;
            }
            ptr::copy_nonoverlapping(name, output, bytes.len());
            ffi::SQLITE_OK
        } else {
            let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
            (*parent).xFullPathname.unwrap()(parent, name, capacity, output)
        }
    }
}
unsafe extern "C" fn delete(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    sync_dir: c_int,
) -> c_int {
    unsafe {
        if is_locator(name) {
            ffi::SQLITE_READONLY
        } else {
            let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
            (*parent).xDelete.unwrap()(parent, name, sync_dir)
        }
    }
}
unsafe extern "C" fn access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    result: *mut c_int,
) -> c_int {
    unsafe {
        if is_locator(name) {
            *result = 0; // Immutable main databases have no journal or WAL paths.
            ffi::SQLITE_OK
        } else {
            let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
            (*parent).xAccess.unwrap()(parent, name, flags, result)
        }
    }
}
unsafe extern "C" fn randomness(
    vfs: *mut ffi::sqlite3_vfs,
    bytes: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
        (*parent).xRandomness.unwrap()(parent, bytes, output)
    }
}
unsafe extern "C" fn sleep(vfs: *mut ffi::sqlite3_vfs, micros: c_int) -> c_int {
    unsafe {
        let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
        (*parent).xSleep.unwrap()(parent, micros)
    }
}
unsafe extern "C" fn current_time(vfs: *mut ffi::sqlite3_vfs, output: *mut f64) -> c_int {
    unsafe {
        let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
        (*parent).xCurrentTime.unwrap()(parent, output)
    }
}
unsafe extern "C" fn last_error(
    vfs: *mut ffi::sqlite3_vfs,
    bytes: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        let parent = (*vfs).pAppData.cast::<ffi::sqlite3_vfs>();
        if let Some(callback) = (*parent).xGetLastError {
            callback(parent, bytes, output)
        } else {
            if bytes > 0 {
                *output = 0;
            }
            0
        }
    }
}

unsafe extern "C" fn dl_open(_: *mut ffi::sqlite3_vfs, _: *const c_char) -> *mut c_void {
    ptr::null_mut()
}
unsafe extern "C" fn dl_error(_: *mut ffi::sqlite3_vfs, bytes: c_int, output: *mut c_char) {
    if bytes > 0 {
        let message = b"Extensions are unavailable for mounted snapshots";
        let length = message.len().min(bytes as usize - 1);
        unsafe {
            ptr::copy_nonoverlapping(message.as_ptr().cast::<c_char>(), output, length);
            *output.add(length) = 0;
        }
    }
}
unsafe extern "C" fn dl_symbol(
    _: *mut ffi::sqlite3_vfs,
    _: *mut c_void,
    _: *const c_char,
) -> Option<unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *mut c_void, *const c_char)> {
    None
}
unsafe extern "C" fn dl_close(_: *mut ffi::sqlite3_vfs, _: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn artifact() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let db = Connection::open(directory.path().join("facts.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE values_test(n INTEGER PRIMARY KEY); INSERT INTO values_test VALUES(7),(11),(23);")
            .unwrap();
        drop(db);
        let frozen = json!({
            "sources": [],
            "rules": [],
            "import_lock": {},
            "import_policy": "declared-ontology-v1",
            "profile_id": "no-rules-v1",
            "parser_options": {},
        });
        fs::write(
            directory.path().join("input_manifest.json"),
            serde_json::to_vec(&frozen).unwrap(),
        )
        .unwrap();
        fs::write(directory.path().join("plan.json"), b"{}").unwrap();
        let mut files = serde_json::Map::new();
        for name in ["facts.sqlite", "input_manifest.json", "plan.json"] {
            let path = directory.path().join(name);
            files.insert(name.to_owned(), json!({"bytes":fs::metadata(&path).unwrap().len(),"sha256":crate::full_artifact::file_digest(&path).unwrap()}));
        }
        let files = Value::Object(files);
        let manifest = json!({"schema_version":"kernel-manifest-v1", "format_version":crate::full_artifact::FORMAT_VERSION,
            "engine_semantic_version":crate::full_artifact::ENGINE_VERSION,"completion":"COMPLETE","kind":"compiled", "profile_id":"no-rules-v1",
            "source_tree_digest":crate::full_artifact::digest(&crate::full_artifact::canonical(&frozen["sources"])),
            "semantic_fingerprint":crate::full_artifact::digest(&crate::full_artifact::canonical(&json!({
                "inputs":frozen,"engine":crate::full_artifact::ENGINE_VERSION,"index_profile":"core-v1"
            }))),
            "content_digest":crate::full_artifact::digest(&crate::full_artifact::canonical(&files)),"files":files});
        fs::write(
            directory.path().join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        directory
    }

    #[test]
    fn snapshot_survives_source_replacement_and_owner_drop() {
        let source = artifact();
        let snapshot = FullSnapshot::mount(source.path(), 1024 * 1024).unwrap();
        assert_eq!(
            snapshot.manifest_sha256(),
            crate::full_artifact::file_digest(&source.path().join("manifest.json")).unwrap()
        );
        let db = open_readonly(Path::new(&snapshot.database_locator())).unwrap();
        // Connections own duplicate descriptors; later requests still use the
        // mounted image even when the application's source path disappears.
        source.close().unwrap();
        let next = open_readonly(Path::new(&snapshot.database_locator())).unwrap();
        drop(snapshot);
        for conn in [db, next] {
            assert_eq!(
                conn.query_row::<i64, _, _>("SELECT SUM(n) FROM values_test", [], |r| r.get(0))
                    .unwrap(),
                41
            );
            assert!(
                conn.execute("INSERT INTO values_test VALUES(99)", [])
                    .is_err()
            );
            let check: String = conn
                .query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .unwrap();
            assert_eq!(check, "ok");
        }
    }

    #[test]
    fn snapshot_mount_rejects_budget_corruption_and_writable_descriptors() {
        let source = artifact();
        assert!(FullSnapshot::mount(source.path(), 1).is_err());
        let named = File::open(source.path().join("facts.sqlite")).unwrap();
        assert!(validate_descriptor(named.as_raw_fd(), named.metadata().unwrap().len()).is_err());
        let writable = tempfile::tempfile().unwrap();
        assert!(validate_descriptor(writable.as_raw_fd(), 0).is_err());
        fs::write(source.path().join("facts.sqlite"), b"corrupt").unwrap();
        assert!(FullSnapshot::mount(source.path(), 1024 * 1024).is_err());
    }

    #[test]
    fn oversized_mount_is_rejected_before_manifest_or_inventory_reads() {
        let source = tempfile::tempdir().unwrap();
        File::create(source.path().join("facts.sqlite"))
            .unwrap()
            .set_len(32 * 1024 * 1024)
            .unwrap();
        fs::write(source.path().join("manifest.json"), b"invalid manifest").unwrap();
        let error = FullSnapshot::mount(source.path(), 1).err().unwrap();
        assert!(error.to_string().starts_with("BUDGET_EXHAUSTED|"));
    }

    #[test]
    fn copying_anonymous_snapshot_keeps_budget_and_digest_checks() {
        let directory = artifact();
        let source = directory.path().join("facts.sqlite");
        let expected = crate::full_artifact::file_digest(&source).unwrap();
        let temporary = tempfile::NamedTempFile::new().unwrap();
        let (mut writer, path) = temporary.into_parts();
        let reader = File::open(&path).unwrap();
        path.close().unwrap();
        // A process exit from this point onwards closes both descriptors and
        // releases all copied bytes without relying on Rust destructors.
        validate_descriptor(reader.as_raw_fd(), 0).unwrap();
        assert!(
            crate::full_artifact::freeze_into(&source, &mut writer, &expected, 1)
                .unwrap_err()
                .to_string()
                .starts_with("BUDGET_EXHAUSTED|")
        );
        assert_eq!(reader.metadata().unwrap().len(), 0);
        crate::full_artifact::freeze_into(&source, &mut writer, &expected, 1024 * 1024).unwrap();
        drop(writer);
        validate_descriptor(reader.as_raw_fd(), fs::metadata(&source).unwrap().len()).unwrap();
        let db = open_readonly(Path::new(&format!("{LOCATOR}{}", reader.as_raw_fd()))).unwrap();
        assert_eq!(
            db.query_row::<i64, _, _>("SELECT SUM(n) FROM values_test", [], |r| r.get(0))
                .unwrap(),
            41
        );
    }

    #[test]
    fn vfs_short_reads_offsets_and_errors_follow_sqlite_contract() {
        let source = artifact();
        let snapshot = FullSnapshot::mount(source.path(), 1024 * 1024).unwrap();
        let mut file = SnapshotFile {
            base: ffi::sqlite3_file { pMethods: &METHODS },
            fd: snapshot.descriptor(),
        };
        let ptr = &mut file.base as *mut _;
        let mut bytes = [0xAAu8; 32];
        unsafe {
            assert_eq!(
                read(
                    ptr,
                    bytes.as_mut_ptr().cast(),
                    32,
                    snapshot.bytes as i64 - 8
                ),
                ffi::SQLITE_IOERR_SHORT_READ
            );
            assert_eq!(&bytes[8..], &[0u8; 24]);
            assert_eq!(read(ptr, bytes.as_mut_ptr().cast(), 32, 0), ffi::SQLITE_OK);
            assert_eq!(&bytes[..16], b"SQLite format 3\0");
            assert_eq!(
                read(ptr, bytes.as_mut_ptr().cast(), 32, -1),
                ffi::SQLITE_IOERR_READ
            );
            assert_eq!(
                read(ptr, bytes.as_mut_ptr().cast(), 32, i64::MAX),
                ffi::SQLITE_IOERR_READ
            );
            assert_eq!(
                read(ptr, bytes.as_mut_ptr().cast(), -1, 0),
                ffi::SQLITE_IOERR_READ
            );
            assert_eq!(
                write(ptr, bytes.as_ptr().cast(), 32, 0),
                ffi::SQLITE_READONLY
            );
            assert_eq!(truncate(ptr, 0), ffi::SQLITE_READONLY);
            (*ptr.cast::<SnapshotFile>()).fd = -1;
            assert_eq!(
                read(ptr, bytes.as_mut_ptr().cast(), 32, 0),
                ffi::SQLITE_IOERR_READ
            );
            let mut size = 0;
            assert_eq!(file_size(ptr, &mut size), ffi::SQLITE_IOERR_FSTAT);
        }
    }

    #[test]
    fn temporary_files_use_the_platform_vfs() {
        let source = artifact();
        let snapshot = FullSnapshot::mount(source.path(), 1024 * 1024).unwrap();
        let db = open_readonly(Path::new(&snapshot.database_locator())).unwrap();
        db.execute_batch("PRAGMA temp_store=FILE; PRAGMA temp.cache_size=10; CREATE TEMP TABLE sorted(n); WITH RECURSIVE numbers(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<100000) INSERT INTO sorted SELECT n FROM numbers;")
            .unwrap();
        let maximum: i64 = db
            .query_row("SELECT n FROM sorted ORDER BY n DESC LIMIT 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(maximum, 100000);
        let timestamp: String = db
            .query_row("SELECT datetime('now')", [], |r| r.get(0))
            .unwrap();
        assert!(!timestamp.is_empty());
    }
}
