#![cfg(windows)]
use nip_native::files::{AttachmentStore, BeginUpload, Limits};
use sha2::{Digest, Sha256};

fn limits() -> Limits {
    Limits {
        max_file_bytes: 4,
        max_server_bytes: 6,
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[test]
#[ignore = "Support process for the crash recovery test"]
fn unfinished_upload_worker() {
    let path = std::path::PathBuf::from(
        std::env::var_os("NIP_FILES_CRASH_PROFILE").expect("worker profile"),
    );
    let store = AttachmentStore::open(path, limits()).unwrap();
    let BeginUpload::Pending(mut upload) = store
        .begin(
            "a".repeat(32),
            "general".into(),
            "partial.bin".into(),
            4,
            hash(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!("expected upload")
    };
    upload.write(b"ho").unwrap();
    // Simulate process loss before commit: no Upload or Connection destructors run.
    std::process::exit(0);
}

#[test]
fn process_restart_discards_unconfirmed_upload_and_releases_reserved_quota() {
    let path = std::env::temp_dir().join(format!(
        "nip-files-crash-{:016x}",
        getrandom::u64().unwrap()
    ));
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "unfinished_upload_worker"])
        .env("NIP_FILES_CRASH_PROFILE", &path)
        .status()
        .unwrap();
    assert!(status.success());
    let store = AttachmentStore::open(path.clone(), limits()).unwrap();
    assert_eq!(store.usage().unwrap().reserved_bytes, 0);
    assert_eq!(store.usage().unwrap().used_bytes, 0);
    let BeginUpload::Pending(upload) = store
        .begin(
            "b".repeat(32),
            "general".into(),
            "retry.bin".into(),
            4,
            hash(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!("expected fresh reservation")
    };
    upload.cancel().unwrap();
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn simultaneous_reservations_accept_exact_quota_and_reject_one_byte_more() {
    let path = std::env::temp_dir().join(format!(
        "nip-files-boundary-{:016x}",
        getrandom::u64().unwrap()
    ));
    let store = AttachmentStore::open(path.clone(), limits()).unwrap();
    let BeginUpload::Pending(a) = store
        .begin(
            "a".repeat(32),
            "general".into(),
            "a.bin".into(),
            4,
            hash(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!()
    };
    let BeginUpload::Pending(mut b) = store
        .begin(
            "b".repeat(32),
            "general".into(),
            "b.bin".into(),
            2,
            hash(b"ab"),
            true,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(store.usage().unwrap().reserved_bytes, 6);
    assert!(
        store
            .begin(
                "c".repeat(32),
                "general".into(),
                "c.bin".into(),
                1,
                hash(b"c"),
                true
            )
            .is_err()
    );
    assert!(
        b.write(b"abc").is_err(),
        "declared file length is enforced before writing excess bytes"
    );
    a.cancel().unwrap();
    b.cancel().unwrap();
    assert_eq!(store.usage().unwrap().reserved_bytes, 0);
    drop(store);
    std::fs::remove_dir_all(path).unwrap();
}
