#![cfg(windows)]
use nip_native::{
    contracts::ChatMessage,
    files::{AttachmentStore, BeginUpload, Limits},
};
use sha2::{Digest, Sha256};

fn root() -> std::path::PathBuf {
    std::env::temp_dir().join(format!("nip-files-{:016x}", getrandom::u64().unwrap()))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn message() -> ChatMessage {
    ChatMessage {
        id: 1,
        sender_id: 1,
        sender_name: "QA".into(),
        channel_id: "general".into(),
        text: "Archivo".into(),
        sent_at: 1,
        attachments: vec![],
    }
}

#[test]
fn durable_upload_is_idempotent_and_downloads_after_restart() {
    let root = root();
    let store = AttachmentStore::open(
        root.clone(),
        Limits {
            max_file_bytes: 4,
            max_server_bytes: 8,
        },
    )
    .unwrap();
    let op = "a".repeat(32);
    let BeginUpload::Pending(mut upload) = store
        .begin(
            op.clone(),
            "general".into(),
            "archivo.txt".into(),
            4,
            digest(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!()
    };
    upload.write(b"ho").unwrap();
    upload.write(b"la").unwrap();
    let attachment = upload.commit(message()).unwrap();
    assert_eq!(store.pending_publications().unwrap().len(), 1);
    assert_eq!(
        store.publication(&op).unwrap().unwrap().attachments[0].id,
        attachment.id
    );
    drop(store);
    let store = AttachmentStore::open(
        root.clone(),
        Limits {
            max_file_bytes: 4,
            max_server_bytes: 8,
        },
    )
    .unwrap();
    let BeginUpload::Committed(retried) = store
        .begin(
            op,
            "general".into(),
            "archivo.txt".into(),
            4,
            digest(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(retried.id, attachment.id);
    store.mark_published(&attachment.id).unwrap();
    assert!(store.pending_publications().unwrap().is_empty());
    let (_, mut file) = store.open_attachment("general", &attachment.id).unwrap();
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
    assert_eq!(bytes, b"hola");
    drop(file);
    assert!(store.open_attachment("other", &attachment.id).is_err());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn quotas_include_concurrent_reservations_and_cancellation_releases_them() {
    let root = root();
    let store = AttachmentStore::open(
        root.clone(),
        Limits {
            max_file_bytes: 4,
            max_server_bytes: 6,
        },
    )
    .unwrap();
    let BeginUpload::Pending(upload) = store
        .begin(
            "a".repeat(32),
            "general".into(),
            "a".into(),
            4,
            digest(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(store.usage().unwrap().reserved_bytes, 4);
    assert!(
        store
            .begin(
                "b".repeat(32),
                "general".into(),
                "b".into(),
                3,
                digest(b"abc"),
                true
            )
            .is_err()
    );
    assert!(
        store
            .begin(
                "c".repeat(32),
                "general".into(),
                "c".into(),
                5,
                digest(b"abcde"),
                true
            )
            .is_err()
    );
    drop(upload);
    assert_eq!(store.usage().unwrap().reserved_bytes, 0);
    assert!(
        store
            .begin(
                "d".repeat(32),
                "general".into(),
                "../escape".into(),
                1,
                digest(b"a"),
                true
            )
            .is_err()
    );
    assert!(
        store
            .begin(
                "e".repeat(32),
                "general".into(),
                "a".into(),
                1,
                digest(b"a"),
                false
            )
            .is_err()
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_integrity_releases_quota_and_channel_deletion_preserves_other_files() {
    let root = root();
    let store = AttachmentStore::open(
        root.clone(),
        Limits {
            max_file_bytes: 4,
            max_server_bytes: 8,
        },
    )
    .unwrap();
    let BeginUpload::Pending(mut bad) = store
        .begin(
            "a".repeat(32),
            "general".into(),
            "bad".into(),
            4,
            digest(b"hola"),
            true,
        )
        .unwrap()
    else {
        panic!()
    };
    bad.write(b"otro").unwrap();
    assert!(bad.commit(message()).is_err());
    assert_eq!(store.usage().unwrap().used_bytes, 0);
    assert_eq!(store.usage().unwrap().reserved_bytes, 0);
    let mut kept = None;
    for (operation, channel) in [("b", "general"), ("c", "other")] {
        let BeginUpload::Pending(mut upload) = store
            .begin(
                operation.repeat(32),
                channel.into(),
                "file".into(),
                4,
                digest(b"hola"),
                true,
            )
            .unwrap()
        else {
            panic!()
        };
        upload.write(b"hola").unwrap();
        let mut publication = message();
        publication.channel_id = channel.into();
        let attachment = upload.commit(publication).unwrap();
        store.mark_published(&attachment.id).unwrap();
        if channel == "other" {
            kept = Some(attachment);
        }
    }
    store
        .set_limits(Limits {
            max_file_bytes: 1,
            max_server_bytes: 1,
        })
        .unwrap();
    assert_eq!(store.usage().unwrap().used_bytes, 8);
    assert!(
        store
            .begin(
                "d".repeat(32),
                "general".into(),
                "file".into(),
                1,
                digest(b"h"),
                true
            )
            .is_err()
    );
    store.delete_channel("general").unwrap();
    assert_eq!(store.usage().unwrap().used_bytes, 4);
    let kept = kept.unwrap();
    drop(store.open_attachment("other", &kept.id).unwrap());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
